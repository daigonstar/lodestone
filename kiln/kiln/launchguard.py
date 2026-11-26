"""Launch guard: make sure a game starts modded, then confirm that it did.

Before launch it checks (and where possible fixes):
  - the deployment is intact (Steam updates / "verify integrity" can remove files);
  - the mod loaders your mods need are installed (CET, RED4ext, redscript, SMAPI,
    SKSE, F4SE);
  - Steam's launch options load them (Proton DLL overrides, SMAPI / script
    extender wrappers);
  - no files in the game folder still point into another mod manager's folder.
After launch it watches each loader's log and reports whether it started.
"""
from __future__ import annotations

import json
import os
import re
import shutil
import stat
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING, Callable

from . import deploy, steam, steamcfg

if TYPE_CHECKING:
    from .instance import Instance

NOTICE_MOD_ID = "Kiln in-game notice"


# ------------------------------------------------------------------ helpers
def _enabled_paths(inst: "Instance") -> list[str]:
    """Lowercased relative paths provided by enabled mods (deploy-root relative)."""
    out = []
    for m in inst.mods:
        if not m.enabled or m.id == NOTICE_MOD_ID:
            continue
        out += [f.lower() for f in inst.mod_files(m)]
    return out


def _staged_form(inst: "Instance", game_rel: str) -> str:
    """Game-folder path → how a mod stages it (deploy-relative, or __root__/...)."""
    root = inst.game.deploy_root.strip("/").lower()
    g = game_rel.lower()
    if not root:
        return g
    if g.startswith(root + "/"):
        return g[len(root) + 1:]
    from .deploy import ROOT_PREFIX
    return ROOT_PREFIX + g


def _exists_ci(base: Path, rel: str) -> bool:
    from .archive import ci_resolve
    return ci_resolve(base, rel) is not None


# ------------------------------------------------------------------ loaders
@dataclass
class Loader:
    id: str
    name: str
    files: list[str]                                   # game-folder relative; all must exist
    needed_by: Callable[[list[str]], int]              # enabled mod paths → how many need it
    dlls: list[str] = field(default_factory=list)      # Proton DLL overrides it needs
    wrapper: Callable[["Instance"], str] | None = None # launch-option wrapper before %command%
    log: Callable[["Instance"], Path | None] | None = None
    parse_log: Callable[[str], tuple[bool, str]] | None = None
    get_url: str = ""                                  # where to get it
    provided_by_mod: bool = True                       # can be installed as a normal mod
    applies: Callable[["Instance"], bool] | None = None   # only needed on some game versions
    nexus_file: tuple[int, str] = (0, "")              # (Nexus mod id, word in the file's name)
    why: str = ""                                      # plain-English reason it's needed


def _count(prefix: str, suffix: str = "") -> Callable[[list[str]], int]:
    def f(paths):
        tops = {p[len(prefix):].split("/", 1)[0] for p in paths
                if p.startswith(prefix) and p.endswith(suffix) and len(p) > len(prefix)}
        return len(tops)
    return f


def _newest(d: Path, pattern: str) -> Path | None:
    files = list(d.glob(pattern)) if d.is_dir() else []
    return max(files, key=lambda p: p.stat().st_mtime) if files else None


def _errors(text: str, rx=r"\[(error|critical)\]|\berror\b") -> list[str]:
    return [ln.strip() for ln in text.splitlines() if re.search(rx, ln, re.I)][:5]


def _cet_log(inst):
    return inst.game_dir / "bin/x64/plugins/cyber_engine_tweaks/cyber_engine_tweaks.log"


def _cet_parse(text):
    mods = len(re.findall(r"Mod (.+?) loaded", text))
    errs = _errors(text, r"\[error\]|\[critical\]")
    return (not errs or mods > 0), (f"{mods} Lua mod(s) loaded" if mods else "started") + (
        f"; {len(errs)} error(s)" if errs else "")


def _red4ext_parse(text):
    loaded = len(re.findall(r"(?i)loaded (?:plugin )?\S+|successfully loaded", text))
    errs = _errors(text, r"\[error\]|\[critical\]")
    return not errs, (f"{loaded} plugin(s) loaded" if loaded else "started") + (
        f"; error: {errs[0][:90]}" if errs else "")


def _smapi_log(_inst):
    for base in (Path(os.environ.get("XDG_CONFIG_HOME") or Path.home() / ".config"),):
        f = base / "StardewValley/ErrorLogs/SMAPI-latest.txt"
        if f.parent.is_dir():
            return f
    return None


def _smapi_parse(text):
    m = re.search(r"Loaded (\d+) mods", text)
    skipped = re.search(r"Skipped mods.*?\n((?:.*\n){1,40}?)\s*\n", text)
    n_skip = len(re.findall(r"^\s*\[.*?\]\s+-\s", skipped.group(1), re.M)) if skipped else 0
    if not m:
        return False, "SMAPI started but hasn't finished loading mods yet"
    return True, f"{m.group(1)} mods loaded" + (f"; {n_skip} skipped (see SMAPI log)" if n_skip else "")


def _se_log(folder, name):
    def f(inst):
        u = steam.prefix_user_dir(inst.game.steam_appid, inst.game_dir)
        return u / "Documents/My Games" / folder / name if u else None
    return f


def _se_parse(text):
    plugins = len(re.findall(r"(?i)plugin .* loaded|loaded correctly", text))
    return True, f"{plugins} plugin(s) loaded" if plugins else "started"


LOADERS: dict[str, list[Loader]] = {
    "cyberpunk2077": [
        Loader("cet", "Cyber Engine Tweaks", ["bin/x64/version.dll", "bin/x64/plugins/cyber_engine_tweaks.asi"],
               _count("bin/x64/plugins/cyber_engine_tweaks/mods/"), dlls=["version"],
               log=_cet_log, parse_log=_cet_parse,
               get_url="https://www.nexusmods.com/cyberpunk2077/mods/107"),
        Loader("red4ext", "RED4ext", ["bin/x64/winmm.dll", "red4ext/RED4ext.dll"],
               _count("red4ext/plugins/"), dlls=["winmm"],
               log=lambda i: _newest(i.game_dir / "red4ext/logs", "red4ext-*.log"),
               parse_log=_red4ext_parse, get_url="https://www.nexusmods.com/cyberpunk2077/mods/2380"),
        Loader("redscript", "redscript", ["engine/tools/scc.exe", "engine/config/base/scripts.ini"],
               _count("r6/scripts/"),
               log=lambda i: _newest(i.game_dir / "r6/logs", "redscript_rCURRENT.log"),
               parse_log=lambda t: (not _errors(t, r"\[ERROR\]"),
                                    "scripts compiled" if not _errors(t, r"\[ERROR\]")
                                    else "compile error: " + _errors(t, r"\[ERROR\]")[0][:90]),
               get_url="https://www.nexusmods.com/cyberpunk2077/mods/1511"),
    ],
    "stardewvalley": [
        Loader("smapi", "SMAPI", ["StardewModdingAPI", "StardewModdingAPI.dll"],
               lambda paths: len({p.split("/", 1)[0] for p in paths if p.endswith("manifest.json")}),
               wrapper=lambda i: '"' + str(i.game_dir / "StardewModdingAPI") + '"',
               log=_smapi_log, parse_log=_smapi_parse,
               get_url="https://github.com/Pathoschild/SMAPI/releases/latest", provided_by_mod=False),
    ],
    "skyrimse": [
        Loader("skse", "SKSE", ["skse64_loader.exe"], _count("skse/plugins/", ".dll"),
               wrapper=lambda i: "bash -c 'exec \"${@/SkyrimSELauncher.exe/skse64_loader.exe}\"' --",
               log=_se_log("Skyrim Special Edition/SKSE", "skse64.log"), parse_log=_se_parse,
               get_url="https://skse.silverlock.org/", provided_by_mod=False),
        Loader("efpre", "Engine Fixes preloader (Part 2)", ["d3dx9_42.dll"],
               lambda paths: int("skse/plugins/enginefixes.dll" in paths), dlls=["d3dx9_42"],
               get_url="https://www.nexusmods.com/skyrimspecialedition/mods/17230?tab=files",
               applies=lambda i: _game_version(i) < (1, 7, 99), nexus_file=(17230, "preloader"),
               why="SSE Engine Fixes needs its Part 2 (d3dx9_42.dll, tbb.dll and tbbmalloc.dll next to "
                   "SkyrimSE.exe) on game versions before 1.7.99. Without it Skyrim shows "
                   "“Engine Fixes did not pre-load” and closes."),
    ],
    "fallout4": [
        Loader("f4se", "F4SE", ["f4se_loader.exe"], _count("f4se/plugins/", ".dll"),
               wrapper=lambda i: "bash -c 'exec \"${@/Fallout4Launcher.exe/f4se_loader.exe}\"' --",
               log=_se_log("Fallout4/F4SE", "f4se.log"), parse_log=_se_parse,
               get_url="https://f4se.silverlock.org/", provided_by_mod=False),
    ],
}


def _se_wrapper(launcher: str, loader: str):
    return lambda i: f"bash -c 'exec \"${{@/{launcher}/{loader}}}\"' --"


def _any_prefix(*prefixes: str, suffix: str = "") -> Callable[[list[str]], int]:
    def f(paths):
        tops = set()
        for p in paths:
            for pre in prefixes:
                if p.startswith(pre) and p.endswith(suffix) and len(p) > len(pre):
                    tops.add(pre + p[len(pre):].split("/", 1)[0])
        return len(tops)
    return f


def _refw(url):
    return Loader("reframework", "REFramework", ["dinput8.dll"], _any_prefix("natives/", "reframework/plugins/"),
                  dlls=["dinput8"], get_url=url)


def _bepinex(url):
    return Loader("bepinex", "BepInEx", ["winhttp.dll"], _count("bepinex/plugins/"), dlls=["winhttp"],
                  get_url=url)


def _ue4ss(project, url):
    return Loader("ue4ss", "UE4SS", [f"{project}/Binaries/Win64/dwmapi.dll"],
                  _count(f"{project.lower()}/binaries/win64/ue4ss/mods/"), dlls=["dwmapi"], get_url=url)


LOADERS.update({
    "skyrim": [Loader("skse", "SKSE", ["skse_loader.exe"], _count("skse/plugins/", ".dll"),
                      wrapper=_se_wrapper("SkyrimLauncher.exe", "skse_loader.exe"),
                      get_url="https://skse.silverlock.org/", provided_by_mod=False)],
    "newvegas": [Loader("nvse", "xNVSE", ["nvse_loader.exe"], _count("nvse/plugins/", ".dll"),
                        wrapper=_se_wrapper("FalloutNVLauncher.exe", "nvse_loader.exe"),
                        get_url="https://github.com/xNVSE/NVSE/releases")],
    "fallout3": [Loader("fose", "FOSE", ["fose_loader.exe"], _count("fose/plugins/", ".dll"),
                        get_url="https://fose.silverlock.org/")],
    "oblivion": [Loader("obse", "xOBSE", ["obse_loader.exe"], _count("obse/plugins/", ".dll"),
                        get_url="https://github.com/llde/xOBSE/releases")],
    "starfield": [Loader("sfse", "SFSE", ["sfse_loader.exe"], _count("sfse/plugins/", ".dll"),
                         wrapper=_se_wrapper("Starfield.exe", "sfse_loader.exe"),
                         get_url="https://sfse.silverlock.org/", provided_by_mod=False)],
    "residentevil42023": [_refw("https://www.nexusmods.com/residentevil42023/mods/12")],
    "residentevil22019": [_refw("https://github.com/praydog/REFramework/releases")],
    "monsterhunterrise": [_refw("https://www.nexusmods.com/monsterhunterrise/mods/26")],
    "monsterhunterwilds": [_refw("https://github.com/praydog/REFramework/releases")],
    "monsterhunterworld": [Loader("stracker", "Stracker's Loader", ["dinput8.dll", "loader.dll"],
                                  _count("nativepc/"), dlls=["dinput8"],
                                  get_url="https://www.nexusmods.com/monsterhunterworld/mods/1982")],
    "eldenring": [Loader("eml", "Elden Mod Loader", ["Game/dinput8.dll"], _count("mods/", ".dll"),
                         dlls=["dinput8"], get_url="https://www.nexusmods.com/eldenring/mods/117")],
    "reddeadredemption2": [Loader("asi", "Script Hook RDR2 / ASI loader", ["dinput8.dll"],
                                  lambda paths: sum(1 for p in paths if p.endswith(".asi")) + _count("lml/")(paths),
                                  dlls=["dinput8"], get_url="https://www.dev-c.com/rdr2/scripthookrdr2/")],
    "subnautica": [_bepinex("https://www.nexusmods.com/subnautica/mods/1108")],
    "valheim": [_bepinex("https://www.nexusmods.com/valheim/mods/3605")],
    "tcgcardshopsimulator": [_bepinex("https://github.com/BepInEx/BepInEx/releases")],
    "palworld": [_ue4ss("Pal", "https://www.nexusmods.com/palworld/mods/2237")],
    "stellarblade": [_ue4ss("SB", "https://github.com/UE4SS-RE/RE-UE4SS/releases")],
    "readyornot": [_ue4ss("ReadyOrNot", "https://github.com/UE4SS-RE/RE-UE4SS/releases")],
    "stalker2heartofchornobyl": [_ue4ss("Stalker2", "https://github.com/UE4SS-RE/RE-UE4SS/releases")],
    "oblivionremastered": [_ue4ss("OblivionRemastered", "https://github.com/UE4SS-RE/RE-UE4SS/releases")],
})


def _game_version(inst) -> tuple[int, ...]:
    from . import pe
    v = pe.file_version(inst.game_dir / inst.game.exe) if inst.game.exe else None
    return tuple(v[:3]) if v else (99, 0, 0)


def needed_loaders(inst: "Instance") -> list[tuple[Loader, int]]:
    """Loaders the enabled mods need, with how many mods need each. Loader rules
    are written relative to the game's deploy folder (Mods/ for Stardew)."""
    paths = _enabled_paths(inst)
    pset = set(paths)
    notice = inst.get(NOTICE_MOD_ID)
    out = []
    for ld in LOADERS.get(inst.game.id, []):
        if ld.applies is not None and not ld.applies(inst):
            continue
        n = ld.needed_by(paths)
        # a loader you installed as a mod should load even if nothing else uses it yet
        provided = bool(ld.files) and all(_staged_form(inst, f) in pset for f in ld.files)
        if ld.id == "cet" and notice and notice.enabled:
            n += 1          # Kiln's in-game notice runs on CET
        # ...and so should one that's installed in the game folder (you put it there to use it)
        if n or provided or loader_installed(inst, ld):
            out.append((ld, n))
    return out


def loader_installed(inst: "Instance", ld: Loader) -> bool:
    return all(_exists_ci(inst.game_dir, f) for f in ld.files)


def wanted_launch_options(inst: "Instance", current: str) -> str:
    opts = current
    dlls = [d for ld, _ in needed_loaders(inst) for d in ld.dlls]
    if dlls:
        opts = steamcfg.with_dll_overrides(opts, dlls)
    for ld, _ in needed_loaders(inst):
        if ld.wrapper:
            opts = steamcfg.with_wrapper(opts, ld.wrapper(inst))
    return opts


# ------------------------------------------------------------------ deployment integrity
def verify_deployment(inst: "Instance") -> list[str]:
    """Deployed files that are missing or no longer the file Kiln placed."""
    m = deploy._load_manifest(inst)
    root = inst.deploy_dir
    bad = []
    for ent in m["files"].values():
        t = root / ent["path"]
        try:
            st = t.lstat()
        except FileNotFoundError:
            bad.append(ent["path"])
            continue
        if ent.get("mode") == "link" and st.st_ino != ent.get("ino"):
            bad.append(ent["path"])
    return bad


def foreign_links(inst: "Instance", limit: int = 5000) -> list[Path]:
    """Symlinks in the game folder that point outside it (left by other mod managers)."""
    out = []
    gd = inst.game_dir.resolve()
    staging = inst.staging.resolve()
    for dirpath, dirs, files in os.walk(inst.game_dir):
        for n in files + dirs:
            p = Path(dirpath) / n
            if p.is_symlink():
                try:
                    tgt = (p.parent / os.readlink(p)).resolve()
                except OSError:
                    continue
                if not tgt.is_relative_to(gd) and not tgt.is_relative_to(staging):
                    out.append(p)
                    if len(out) >= limit:
                        return out
    return out


def materialize_links(links: list[Path]) -> tuple[int, int]:
    """Replace symlinks with real copies of what they point to (broken ones are removed)."""
    fixed = removed = 0
    for p in links:
        try:
            tgt = p.resolve(strict=True)
        except (FileNotFoundError, RuntimeError):
            p.unlink(missing_ok=True)
            removed += 1
            continue
        tmp = p.with_name(p.name + ".kiln-tmp")
        if tgt.is_dir():
            shutil.copytree(tgt, tmp, symlinks=False)
        else:
            shutil.copy2(tgt, tmp)
        p.unlink()
        tmp.rename(p)
        fixed += 1
    return fixed, removed


# ------------------------------------------------------------------ checks
@dataclass
class Check:
    level: str                  # "ok", "warn", "error"
    title: str
    detail: str = ""
    fix: str = ""               # fix id understood by the GUI
    fix_label: str = ""
    data: dict = field(default_factory=dict)


def preflight(inst: "Instance", pending_deploy: bool = False) -> list[Check]:
    out: list[Check] = []
    enabled = [m for m in inst.mods if m.enabled]
    # 1. deployment
    if enabled and (pending_deploy or not deploy.is_deployed(inst)):
        out.append(Check("warn", "Mods aren't fully deployed", "Kiln will deploy before launching.",
                         "deploy", "Deploy now"))
    else:
        bad = verify_deployment(inst)
        if bad:
            out.append(Check("error", f"{len(bad)} deployed file(s) are missing or were replaced",
                             "A Steam update or “Verify integrity” probably removed them, e.g. "
                             + ", ".join(bad[:3]), "deploy", "Redeploy"))
        elif enabled:
            out.append(Check("ok", f"{len(enabled)} mods deployed and intact"))
    # 2. leftovers from other managers
    links = foreign_links(inst, 2000)
    if links:
        out.append(Check("warn", f"{len(links)} file(s) in the game folder are links into another "
                         "mod manager's folder",
                         "If that manager purges or moves them, the game breaks. e.g. "
                         + str(links[0].relative_to(inst.game_dir)), "materialize",
                         "Turn into real files", {"links": [str(p) for p in links]}))
    # 3. loaders
    needs = needed_loaders(inst)
    for ld, n in needs:
        if loader_installed(inst, ld):
            out.append(Check("ok", f"{ld.name} is installed" + (f" ({n} mod(s) use it)" if n else "")))
        else:
            out.append(Check("error", f"{ld.name} is needed by {n} mod(s) but isn't installed",
                             ld.why or f"Without it those mods do nothing. Get it from {ld.get_url}",
                             f"loader:{ld.id}",
                             "Install…" if ld.id == "smapi" or ld.nexus_file[0] else "Get it…",
                             {"url": ld.get_url, "nexus_file": list(ld.nexus_file), "why": ld.why}))
    # 4. Steam launch options
    if needs:
        cur = steamcfg.get_launch_options(inst.game.steam_appid)
        want = wanted_launch_options(inst, cur)
        if want != cur:
            out.append(Check("error", "Steam's launch options don't load the mod loaders",
                             f"Now: {cur or '(empty)'}\nNeeded: {want}", "launch_options",
                             "Fix launch options", {"current": cur, "wanted": want}))
        else:
            out.append(Check("ok", "Steam launch options load the mod loaders", cur))
    # 5. mods whose required mods are missing (Stardew manifests)
    from . import smapideps
    miss = smapideps.check(inst)
    if miss:
        skipped = sorted({n for m in miss for n in m.needed_by})
        out.append(Check("error", f"{len(skipped)} mod(s) will be skipped: they need mods you don't have",
                         "Missing: " + ", ".join(m.unique_id for m in miss[:4])
                         + ("…" if len(miss) > 4 else "") + "\nSkipped: " + ", ".join(skipped[:5])
                         + ("…" if len(skipped) > 5 else ""), "deps", "Install missing…"))
    # 6. plugins (Bethesda)
    if inst.game.has_plugins:
        miss = inst.missing_master_warnings()
        if miss:
            out.append(Check("warn", f"{len(miss)} plugin(s) are missing a master",
                             "\n".join(miss[:4]), "plugins", "Show plugins"))
    return out


# ------------------------------------------------------------------ after launch
@dataclass
class LoaderResult:
    loader: Loader
    started: bool = False
    ok: bool = False
    summary: str = "waiting…"


def check_logs(inst: "Instance", since: float) -> list[LoaderResult]:
    res = []
    for ld, _n in needed_loaders(inst):
        r = LoaderResult(ld)
        f = ld.log(inst) if ld.log else None
        if f and f.is_file() and f.stat().st_mtime >= since - 2:
            r.started = True
            try:
                text = f.read_text(encoding="utf-8", errors="replace")
            except OSError:
                text = ""
            r.ok, r.summary = ld.parse_log(text) if ld.parse_log else (True, "started")
        res.append(r)
    return res


def game_running(inst: "Instance") -> bool:
    """Is a process from the game folder running? (works for native and Proton games)"""
    gd = str(inst.game_dir)
    exe = Path(inst.game.exe).name.lower()
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            cmd = Path(f"/proc/{pid}/cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace")
        except OSError:
            continue
        low = cmd.lower()
        if (gd in cmd or exe in low) and "kiln" not in low and "steam.sh" not in low:
            if any(x in low for x in (exe, "stardewmoddingapi", "stardew valley", "skse64_loader",
                                      "f4se_loader")):
                return True
    return False


# ------------------------------------------------------------------ in-game notice (CET)
CET_NOTICE = r'''-- Written by Kiln. Shows which mods Kiln loaded, then gets out of the way.
local INFO = %(info)s
local t, warned = 0, false

registerForEvent("onUpdate", function(dt)
    t = t + dt
    if not warned then
        local ok, player = pcall(function() return Game.GetPlayer() end)
        if ok and player then
            warned = true
            pcall(function() player:SetWarningMessage(INFO.short) end)
        end
    end
end)

registerForEvent("onDraw", function()
    if t > INFO.seconds then return end
    ImGui.SetNextWindowPos(28, 28, ImGuiCond.Always)
    ImGui.SetNextWindowBgAlpha(0.82)
    local flags = ImGuiWindowFlags.NoTitleBar + ImGuiWindowFlags.AlwaysAutoResize
                + ImGuiWindowFlags.NoInputs + ImGuiWindowFlags.NoSavedSettings
    ImGui.Begin("Kiln##notice", flags)
    ImGui.TextColored(1.0, 0.6, 0.24, 1.0, "Kiln")
    for _, line in ipairs(INFO.lines) do ImGui.Text(line) end
    ImGui.End()
end)
'''


def _lua(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", " ") + '"'


def notice_text(inst: "Instance") -> tuple[str, list[str]]:
    on = [m for m in inst.mods if m.enabled and m.id != NOTICE_MOD_ID]
    loaders = [ld.name for ld, _ in needed_loaders(inst)]
    short = f"Kiln: {len(on)} mods active ({inst.active_profile})"
    lines = [f"{len(on)} mods active · profile: {inst.active_profile}",
             ("Loaders: " + ", ".join(loaders)) if loaders else "",
             f"Deployed {time.strftime('%d %b %H:%M')}"]
    return short, [ln for ln in lines if ln]


def ensure_notice_mod(inst: "Instance") -> bool:
    """Keep a tiny CET mod in staging that shows a 'mods loaded' box in game.
    Returns True if it was created (needs a deploy)."""
    if inst.game.id != "cyberpunk2077":
        return False
    from .instance import Mod
    rel = "bin/x64/plugins/cyber_engine_tweaks/mods/kiln_notice/init.lua"
    m = inst.get(NOTICE_MOD_ID)
    created = False
    if m is None:
        m = Mod(id=NOTICE_MOD_ID, name="Kiln in-game notice", author="Kiln", category="Utilities",
                installed_at=int(time.time()),
                summary="Shows a short “mods loaded” box when the game starts (needs CET). "
                        "Disable it to hide the box.")
        inst.mods.insert(0, m)
        created = True
    short, lines = notice_text(inst)
    info = ("{ short = %s, seconds = 20, lines = { %s } }"
            % (_lua(short), ", ".join(_lua(x) for x in lines)))
    f = inst.mod_path(m) / rel
    f.parent.mkdir(parents=True, exist_ok=True)
    body = CET_NOTICE % {"info": info}
    if f.exists():
        with open(f, "r+", encoding="utf-8") as fh:   # rewrite in place: keeps the hardlink
            fh.seek(0)
            fh.write(body)
            fh.truncate()
    else:
        f.write_text(body, encoding="utf-8")
        inst.forget_files(NOTICE_MOD_ID)
    inst.save()
    return created


# ------------------------------------------------------------------ installing SMAPI
def install_smapi(inst: "Instance", installer_zip: Path) -> list[str]:
    """Install SMAPI from its official installer zip without running the installer:
    unpack internal/linux/install.dat into the game folder. Returns installed files."""
    import zipfile
    with zipfile.ZipFile(installer_zip) as z:
        dat = next((n for n in z.namelist() if n.replace("\\", "/").endswith("internal/linux/install.dat")), None)
        if not dat:
            raise RuntimeError("That isn't a SMAPI installer zip (no internal/linux/install.dat).")
        import io
        inner = zipfile.ZipFile(io.BytesIO(z.read(dat)))
        names = []
        gd = inst.game_dir.resolve()
        for info in inner.infolist():
            rel = info.filename.replace("\\", "/")
            if info.is_dir() or not rel or rel.startswith("/") or ".." in rel.split("/"):
                continue
            target = gd / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            with inner.open(info) as src, open(target, "wb") as dst:
                shutil.copyfileobj(src, dst)
            names.append(rel)
    # like SMAPI's installer: SMAPI runs on the game's own .NET dependency list
    deps = gd / "Stardew Valley.deps.json"
    if deps.is_file():
        shutil.copyfile(deps, gd / "StardewModdingAPI.deps.json")
        names.append("StardewModdingAPI.deps.json")
    for exe in ("StardewModdingAPI",):
        p = gd / exe
        if p.exists():
            p.chmod(p.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    rec = inst.staging / "tools.json"
    try:
        tools = json.loads(rec.read_text())
    except (OSError, ValueError):
        tools = {}
    tools["smapi"] = {"files": names, "installed_at": int(time.time()), "from": installer_zip.name}
    rec.write_text(json.dumps(tools, indent=1))
    return names


def smapi_release() -> tuple[str, str, int]:
    """(version, download url, size) of the latest SMAPI installer on GitHub."""
    import requests
    r = requests.get("https://api.github.com/repos/Pathoschild/SMAPI/releases/latest", timeout=20,
                     headers={"Accept": "application/vnd.github+json", "User-Agent": "Kiln"})
    r.raise_for_status()
    d = r.json()
    a = next((a for a in d.get("assets", []) if re.search(r"installer\.zip$", a["name"])
              and "developer" not in a["name"].lower()), None)
    if not a:
        raise RuntimeError("The latest SMAPI release has no installer zip.")
    return d.get("tag_name", "?"), a["browser_download_url"], int(a.get("size") or 0)
