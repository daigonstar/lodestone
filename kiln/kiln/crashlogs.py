"""Read the logs and crash reports games and mod loaders leave behind, and turn
them into plain-English Issues for the doctor.

  Skyrim:     crash logs from Crash Logger SSE / Trainwreck / .NET Script Framework,
              and SKSE's own log (plugins that failed to load).
  Stardew:    SMAPI's log (mods skipped and why, mods throwing errors).
  Cyberpunk:  redscript's log (script mods that failed to compile).
"""
from __future__ import annotations

import os
import re
import time
from collections import Counter
from pathlib import Path
from typing import TYPE_CHECKING

from . import steam

if TYPE_CHECKING:
    from .doctor import Ctx

RECENT = 14 * 86400          # crash reports older than this are ignored


def _read(p: Path, limit: int = 4 << 20) -> str:
    try:
        with open(p, "rb") as fh:
            return fh.read(limit).decode("utf-8", "replace")
    except OSError:
        return ""


def _when(p: Path) -> str:
    t = p.stat().st_mtime
    days = (time.time() - t) / 86400
    day = "today" if days < 1 and time.localtime(t).tm_mday == time.localtime().tm_mday else \
        time.strftime("%a %d %b", time.localtime(t))
    return f"{day} at {time.strftime('%H:%M', time.localtime(t))}"


def _mygames(ctx) -> Path | None:
    """The game's Documents/My Games folder (takes a doctor Ctx or an Instance)."""
    inst = getattr(ctx, "inst", ctx)
    g = inst.game
    folder = getattr(g, "mygames_name", "")
    u = steam.prefix_user_dir(g.steam_appid, inst.game_dir)
    return u / "Documents/My Games" / folder if (u and folder) else None


# ====================================================================== Skyrim crash logs
SYSTEM = {"kernel32.dll", "kernelbase.dll", "ntdll.dll", "ucrtbase.dll", "msvcp140.dll",
          "vcruntime140.dll", "vcruntime140_1.dll", "user32.dll", "win32u.dll", "combase.dll",
          "steam_api64.dll", "msvcrt.dll", "kernel32", "ntdll"}
GRAPHICS = {"d3d11.dll", "dxgi.dll", "d3d12.dll", "nvwgf2umx.dll", "nvldumdx.dll", "amdxx64.dll",
            "atidxx64.dll", "igd10iumd64.dll", "vulkan-1.dll", "d3d9.dll", "winevulkan.dll",
            "d3dcompiler_47.dll"}
BASE_PLUGINS = {"skyrim.esm", "update.esm", "dawnguard.esm", "hearthfires.esm", "dragonborn.esm",
                "fallout4.esm", "dlcrobot.esm", "dlcworkshop01.esm", "dlccoast.esm",
                "dlcworkshop02.esm", "dlcworkshop03.esm", "dlcnukaworld.esm"}
KNOWN_DLLS = {
    "hdtsmp64.dll": "the physics engine for hair, capes and bodies (HDT-SMP / FSMP). Physics "
                    "settings or a physics-enabled outfit or hair is usually the trigger",
    "skee64.dll": "RaceMenu (character appearance). A face/body preset, overlay or a RaceMenu "
                  "version that doesn't match your game is the usual trigger",
    "tbbmalloc.dll": "the memory allocator Engine Fixes installs. It's rarely the real cause: it "
                     "shows up when the game ran out of memory or another mod corrupted it",
    "xaudio2_7.dll": "the game's sound system. A broken or wrongly formatted sound file from a "
                     "mod is the usual cause",
    "jcontainers64.dll": "JContainers, a scripting library used by many mods",
    "po3_papyrusextender.dll": "powerofthree's Papyrus Extender, a scripting library",
    "enginefixes.dll": "SSE Engine Fixes",
    "dynamicanimationreplacer.dll": "Dynamic Animation Replacer, which doesn't support recent "
                                    "game versions",
}


def _section(text: str, head: str) -> str:
    m = re.search(rf"^\s*{head}\s*:?\s*$(.*?)(?=^\s*[A-Z][A-Z ]{{3,}}:?\s*$|\Z)", text, re.M | re.S)
    return m.group(1) if m else ""


def analyze_crash(text: str) -> dict:
    """Pull the useful facts out of a crash log (Crash Logger SSE, Trainwreck or .NET
    Script Framework). All values are plain data; issues() turns them into words."""
    out: dict = {"exception": "", "module": "", "dlls": [], "plugins": [], "nifs": [], "names": []}
    m = re.search(r'Unhandled (?:native )?exception\s*"?([A-Z_]+)"?\s*at\s*0x[0-9A-Fa-f]+\s*\(?([\w.\-]+)?', text)
    if m:
        out["exception"], out["module"] = m.group(1), (m.group(2) or "").lower()
    stack = _section(text, r"PROBABLE CALL STACK") or _section(text, r"CALL STACK") or \
        _section(text, r"Possible relevant objects") or text[:20000]
    seen: dict[str, str] = {}
    for d in re.findall(r"([\w.\-]+\.dll)\+", stack, re.I):
        seen.setdefault(d.lower(), d)
    out["dlls"] = list(seen)
    out["dll_names"] = seen
    objects = (_section(text, "REGISTERS") + _section(text, "STACK")) or text
    files = Counter()
    for f in re.findall(r'File:\s*"([^"]+\.es[mpl])"', objects, re.I) + \
            re.findall(r"Modified by:\s*([^\n]+)", objects):
        for name in re.split(r"\s*->\s*", f):
            name = name.strip().strip('"')
            if name.lower().endswith((".esp", ".esm", ".esl")) and name.lower() not in BASE_PLUGINS \
                    and not name.lower().startswith("cc"):
                files[name] += 1
    out["plugins"] = [n for n, _ in files.most_common(5)]
    out["nifs"] = list(dict.fromkeys(re.findall(r'"?((?:meshes[\\/])?[\w \-\\/.]+\.nif)"?', objects, re.I)))[:3]
    out["names"] = [n for n in dict.fromkeys(re.findall(r'Name:\s*"([^"]+)"', objects))
                    if not re.search(r"\.(nif|dds|hkx|esp|esm|esl|dll|pex)$", n, re.I)][:3]
    for n in re.findall(r'Name:\s*"([^"]+\.nif)"', objects, re.I):
        if n not in out["nifs"]:
            out["nifs"].append(n)
    return out


def _crash_files(ctx: "Ctx") -> list[Path]:
    mg = _mygames(ctx)
    found = []
    if mg:
        for pat in ("SKSE/crash-*.log", "SKSE/Crashlogs/*.log", "F4SE/crash-*.log",
                    "F4SE/Crashlogs/*.log"):
            found += list(mg.glob(pat))
    nsf = ctx.inst.deploy_dir / "NetScriptFramework/Crash"
    if nsf.is_dir():
        found += [p for p in nsf.glob("Crash_*.txt")]
    now = time.time()
    return sorted((p for p in found if now - p.stat().st_mtime < RECENT),
                  key=lambda p: p.stat().st_mtime, reverse=True)


def crash_issues(ctx: "Ctx") -> list:
    from .doctor import Action, Issue, _list, _q
    files = _crash_files(ctx)
    if not files:
        return []
    return crash_issues_for(ctx, files[0], len(files))


def crash_issues_for(ctx: "Ctx", newest: Path, n_files: int = 1) -> list:
    """The doctor's issue for one crash report."""
    from .doctor import Action, Issue, _list, _q
    a = analyze_crash(_read(newest))
    game = ctx.inst.game.name
    plugins_dir = "SKSE/Plugins" if ctx.inst.game.id == "skyrimse" else "F4SE/Plugins"
    suspects, acts, mods = [], [], []
    lines = []
    culprit = next((d for d in a["dlls"] if d not in SYSTEM and not d.endswith(".exe")), "")
    if a["module"] in GRAPHICS or culprit in GRAPHICS:
        lines.append("It crashed inside the graphics driver. That usually means the game ran out "
                     "of video memory (too many or too big texture mods), or a graphics mod "
                     "(ENB, ReShade, Community Shaders) doesn't agree with your setup.")
    elif culprit:
        m = ctx.owner(f"{plugins_dir}/{culprit}")
        what = KNOWN_DLLS.get(culprit, "")
        shown = a.get("dll_names", {}).get(culprit, culprit)
        lines.append(f"It crashed inside {shown}" + (f", which comes from {_q(m.name)}" if m else "")
                     + (f": {what}." if what else "."))
        if m:
            suspects.append(m)
    elif a["dlls"] or a["module"]:
        lines.append("It crashed inside the game itself, so the stack only points at the game. "
                     "The objects it was handling (below) are the best clue.")
    for nif in a["nifs"]:
        rel = nif.replace("\\", "/")
        rel = rel if rel.lower().startswith("meshes/") else "meshes/" + rel
        m = ctx.owner(rel)
        lines.append(f"It was loading the model {rel}" + (f" from {_q(m.name)}" if m else "")
                     + ". A broken or incompatible model (mesh) is a very common crash cause.")
        if m and m not in suspects:
            suspects.append(m)
    plug_mods = []
    for p in a["plugins"][:3]:
        m = ctx.owner(p)
        plug_mods.append(f"{p}" + (f" ({m.name})" if m else ""))
        if m and m not in suspects:
            suspects.append(m)
    if plug_mods:
        lines.append("Things from these plugins were involved: " + _list(plug_mods, 3) + ".")
    if a["names"]:
        lines.append("It involved: " + _list(_q(n) for n in a["names"]) + ".")
    if a["exception"] == "EXCEPTION_STACK_OVERFLOW":
        lines.append("The error is a stack overflow, which is often caused by a mod stuck in a "
                     "loop or a patch made for a different version of the mod it patches.")
    for m in suspects[:2]:
        acts.append(Action(f"disable:{m.id}", f"Turn off {m.name}", auto=False))
        mods.append(m.id)
    acts.append(Action(f"open_file:{newest}", "Open the crash log", auto=False))
    more = f" ({n_files} crash reports in the last two weeks.)" if n_files > 1 else ""
    manifest = ctx.inst.deploy_dir / ".kiln-deployment.json"
    changed = manifest.is_file() and manifest.stat().st_mtime > newest.stat().st_mtime + 5
    if changed:
        more += (" Your mods have changed since this crash, so it may already be fixed: play "
                 "again, and Kiln will tell you if it crashes again.")
    return [Issue(
        f"crash:{newest.name}:{int(newest.stat().st_mtime)}", "warn" if changed else "crash",
        f"{game} crashed {_when(newest)}",
        "Kiln read the crash report your crash logger wrote." + more + "\n\n" + "\n".join(lines),
        ["Fix any other red problems on this page first: they're the most likely cause.",
         "If it keeps crashing in the same place, turn off the mod named above and try again. "
         "If that stops it, look on that mod's Nexus page for an update or a compatibility patch.",
         "Crashes right at the main menu are almost always a missing master or an outdated "
         "SKSE plugin; crashes while playing are usually a specific mesh, script or memory."],
        acts, mods, "crash")]


# ====================================================================== SKSE log
LAST_ERRORS = {
    "126": "it needs another file that isn't there, usually the Microsoft Visual C++ "
           "runtime. Under Proton, install it with: protontricks {appid} vcrun2022",
    "193": "the file is broken or for a different Windows architecture (a 32-bit plugin, or "
           "a damaged download)",
    "127": "it's built against a different SKSE version",
}


def xse_load_failures(ctx: "Ctx") -> list[tuple[str, str]]:
    """[(dll, how)] for plugins SKSE/F4SE failed to start last time ('crashed' while
    loading, or 'error N' from Windows), skipping DLLs changed since that log."""
    mg = _mygames(ctx)
    name = {"skyrimse": "SKSE/skse64.log", "fallout4": "F4SE/f4se.log"}.get(ctx.inst.game.id)
    if not mg or not name or not (mg / name).is_file():
        return []
    log = mg / name
    text = _read(log)
    plugins_dir = name.split("/")[0] + "/Plugins"
    out = []
    for m in re.finditer(r"plugin\s+(\S+?\.dll)\s+\(.*?\)\s+(disabled, fatal error occurred while loading plugin"
                         r"|couldn't load plugin\s+(\d+))", text):
        dll = Path(m.group(1).replace("\\", "/")).name
        p = ctx.inst.deploy_dir / plugins_dir / dll
        if p.is_file() and p.stat().st_mtime > log.stat().st_mtime:
            continue
        out.append((dll, f"error {m.group(3)}" if m.group(3) else "crashed"))
    return out


def xse_log_issues(ctx: "Ctx") -> list:
    from .doctor import Action, Issue, _q
    from . import vcruntime
    if not vcruntime.too_old(ctx.inst):       # with an old runtime, the doctor's runtime issue explains these
        fails = xse_load_failures(ctx)
        by_mod: dict = {}
        plugins_dir = {"skyrimse": "SKSE/Plugins", "fallout4": "F4SE/Plugins"}.get(ctx.inst.game.id, "")
        for dll, how in fails:
            m = ctx.owner(f"{plugins_dir}/{dll}")
            by_mod.setdefault(m.id if m else dll, (m, []))[1].append(dll)
        out = []
        for key, (m, dlls) in by_mod.items():
            out.append(Issue(
                f"xse_crashed:{key}", "broken",
                f"{_q(m.name) if m else dlls[0]} crashed while starting last time you played",
                f"SKSE loaded {', '.join(dlls)}, but it failed while starting up, so SKSE switched it off. "
                "This usually means the plugin was made for a different game version than yours, or "
                "it needs a newer version of a library mod.",
                ["Update the mod (or get the file made for your game version).",
                 "If it keeps failing, turn it off: SKSE already skips it, but mods that depend on it "
                 "may misbehave."],
                ([Action(f"update_compatible:{m.id}", "Find a version that works", auto=False),
                  Action(f"disable:{m.id}", "Turn it off", auto=False)] if m and m.nexus_mod_id else
                 [Action(f"disable:{m.id}", "Turn it off", auto=False)] if m else []),
                [m.id] if m else [], "logs"))
    else:
        out = []
    mg = _mygames(ctx)
    name = {"skyrimse": "SKSE/skse64.log", "fallout4": "F4SE/f4se.log"}.get(ctx.inst.game.id)
    if not mg or not name or not (mg / name).is_file():
        return out
    log = mg / name
    text = _read(log)
    plugins_dir = name.split("/")[0] + "/Plugins"
    out += _startup_stop_issues(ctx, log, text, plugins_dir)
    for m in re.finditer(r"couldn't load plugin\s+(.+?\.dll)\s*\(Last Error:\s*(\d+)", text, re.I):
        dll = Path(m.group(1).replace("\\", "/")).name
        mod = ctx.owner(f"{plugins_dir}/{dll}")
        p = ctx.inst.deploy_dir / plugins_dir / dll
        if p.is_file() and p.stat().st_mtime > log.stat().st_mtime:
            continue                    # replaced since that log was written
        why = LAST_ERRORS.get(m.group(2), f"Windows error {m.group(2)}").format(appid=ctx.inst.game.steam_appid)
        acts = [Action(f"disable:{mod.id}", "Turn it off", auto=False)] if mod else []
        out.append(Issue(
            f"xse_log:{dll}", "broken",
            f"{_q(mod.name) if mod else dll} failed to load last time you played",
            f"{name.split('/')[0]}'s log says it couldn't load {dll}: {why}.",
            ["Reinstall the mod, or get its newest version.",
             "If the log mentions Visual C++, run the protontricks command above once."],
            acts, [mod.id] if mod else [], "logs"))
    for m in re.finditer(r"plugin\s+(.+?\.dll).*?reported as incompatible during (?:query|load)", text, re.I):
        dll = Path(m.group(1).replace("\\", "/")).name
        mod = ctx.owner(f"{plugins_dir}/{dll}")
        out.append(Issue(
            f"xse_refused:{dll}", "broken",
            f"{_q(mod.name) if mod else dll} refused to run on your game version",
            f"{dll} checked the game when it started and decided it isn't compatible, so it "
            "turned itself off.",
            ["Get the newest version of the mod for your game version."],
            [Action(f"update_compatible:{mod.id}", "Find a version that works", auto=False)] if mod and mod.nexus_mod_id else [],
            [mod.id] if mod else [], "logs"))
    return out


CRASH_LOGGER = 59818        # Crash Logger SSE AE VR on Nexus


def _startup_stop_issues(ctx: "Ctx", log: Path, text: str, plugins_dir: str) -> list:
    """The game closed while the script extender was handing a start-up signal to a
    plugin: the log just stops, with no error, because the plugin crashed the game."""
    from .doctor import Action, Issue, _q
    from . import launchguard, logreader
    stop = logreader.startup_stop(ctx.inst, text)
    if not stop or launchguard.game_running(ctx.inst):
        return []
    dll = stop["dll"]
    p = ctx.inst.deploy_dir / plugins_dir / dll
    if not p.is_file() or p.stat().st_mtime > log.stat().st_mtime + 1:
        return []                      # gone or replaced since: wait for the next launch
    m = ctx.owner(f"{plugins_dir}/{dll}")
    if m is None or not m.enabled:
        return []
    known = logreader.KNOWN_STARTUP.get(dll.lower())
    reason = known(ctx.inst) if known else ""
    xse = plugins_dir.split("/")[0]
    acts = [Action(f"disable:{m.id}", f"Turn off {m.name}", auto=bool(reason),
                   describe=f"Turn off {m.name} (it crashes the game at start-up and isn't needed)")]
    if m.nexus_mod_id and not reason:
        acts.append(Action(f"update_compatible:{m.id}", "Find a version that works", auto=False))
    acts.append(Action(f"open_file:{log}", f"Open {xse}'s log", auto=False))
    out = [Issue(
        f"xse_stop:{dll}:{int(log.stat().st_mtime)}", "crash",
        f"{ctx.inst.game.name} closed while {_q(m.name)} was starting",
        f"{xse}'s log stops while it was handing {logreader.MESSAGES.get(stop['message'], 'a signal')} "
        f"to {dll}, from {_q(m.name)}. The game closed at that moment, so that plugin almost certainly "
        "crashed it. Nothing says “error” because a plugin that crashes can't write anything."
        + (f"\n\n{reason}" if reason else ""),
        ["Turn the mod off and start the game again. If the game starts, the mod (or one of its "
         "settings files) was the problem.",
         "Look on the mod's Nexus page (Posts and Bugs tabs) for the same crash, and for a newer version.",
         "If you need the mod, check its requirements: a missing requirement often causes this."],
        acts, [m.id], "logs")]
    if not logreader._has_crash_logger(ctx.inst) and ctx.inst.game.id == "skyrimse":
        out.append(Issue(
            "no_crash_logger", "info", "Install a crash logger so crashes explain themselves",
            "When the game crashes, a crash logger writes down what it was doing. Kiln reads those "
            "reports and tells you which mod caused the crash. Without one, Kiln can only guess from "
            "where the logs stop.",
            ["Install “Crash Logger SSE AE VR” from Nexus (it's a small SKSE plugin)."],
            [Action(f"install_nexus:{CRASH_LOGGER}", "Install Crash Logger", auto=False)], [], "logs"))
    return out


# ====================================================================== SMAPI (Stardew)
SMAPI_REASONS = [
    (r"requires mods which aren't installed|requires .* which isn't installed", None),   # deps check covers it
    (r"newer version of SMAPI|requires SMAPI", ("Needs a newer SMAPI",
        "This mod needs a newer version of SMAPI than you have.", ["Update SMAPI: Kiln does this "
        "from the Dashboard's “Set up essentials…” (or download it from smapi.io)."], "smapi")),
    (r"no longer compatible|incompatible|obsolete", ("Made for an older Stardew version",
        "SMAPI knows this version of the mod is broken with the current game.",
        ["Get the newest version from the mod's page, or turn it off. Many old mods have a "
         "replacement: the mod page usually says which."], "update")),
    (r"XNB mod", ("An XNB mod, which SMAPI can't load",
        "XNB mods replace the game's own files directly. SMAPI (and Kiln) don't support that "
        "kind; they break with every game update.", ["Look for a Content Patcher version of "
        "the mod (search its name plus “CP”), and turn this one off."], "disable")),
    (r"multiple|same unique ID|duplicate", ("Installed twice",
        "There are two copies of this mod, so SMAPI loads neither.",
        ["Keep one copy: turn the older one off in Kiln."], "disable")),
    (r"couldn't be loaded|failed loading|DLL", ("Its code couldn't be loaded",
        "The mod's program file couldn't be loaded. It may be a damaged download or built "
        "for a different version of the game.", ["Reinstall it, or get its newest version."],
        "update")),
    (r"manifest|empty", ("Installed wrong (no manifest)",
        "SMAPI couldn't find the mod's manifest.json, so the files are probably one folder "
        "too deep or incomplete.", ["Reinstall it; if that doesn't help, re-download it."], "reinstall")),
]


def _smapi_names(ctx: "Ctx") -> dict[str, "object"]:
    from .smapideps import read_manifest
    names = {}
    for m in ctx.inst.mods:
        root = ctx.inst.mod_path(m)
        for dirpath, dirs, files in os.walk(root):
            if "manifest.json" in files:
                man = read_manifest(Path(dirpath) / "manifest.json")
                if man.get("name"):
                    names[str(man["name"]).lower()] = m
                dirs[:] = []
    return names


def smapi_issues(ctx: "Ctx") -> list:
    from .doctor import Action, Issue, _q
    from .launchguard import _smapi_log
    f = _smapi_log(ctx.inst)
    if not f or not f.is_file() or time.time() - f.stat().st_mtime > RECENT:
        return []
    text = _read(f)
    out = []
    names = None
    skipped, inside = [], False
    for line in text.splitlines():
        body = re.sub(r"^\[[^\]]*\]", "", line).strip()
        if body.startswith("Skipped mods"):
            inside = True
            continue
        if inside:
            if not body:
                break
            skipped.append(body)
    for line in skipped:
        m = re.search(r"-\s+(.+?)\s+(?:\d[\w.\-]*\s+)?because\s+(.+)", line)
        if not m:
            continue
        mod_name, reason = m.group(1).strip(), m.group(2).strip().rstrip(".")
        for rx, info in SMAPI_REASONS:
            if re.search(rx, reason, re.I):
                break
        else:
            info = ("Skipped by SMAPI", f"SMAPI says: {reason}.", ["Reinstall or update the mod."], "update")
        if info is None:
            continue
        names = names if names is not None else _smapi_names(ctx)
        km = names.get(mod_name.lower())
        title, explain, steps, act = info
        acts = []
        if km and act == "disable":
            acts.append(Action(f"disable:{km.id}", "Turn it off", auto=False))
        elif km and act == "update" and km.nexus_mod_id:
            acts.append(Action(f"update_compatible:{km.id}", "Find the newest version", auto=False))
        elif km and act == "reinstall":
            acts.append(Action(f"reinstall:{km.id}", "Reinstall", auto=False))
        elif act == "smapi":
            acts.append(Action("loader:smapi", "Update SMAPI", auto=False))
        out.append(Issue(f"smapi_skip:{mod_name}", "broken", f"{_q(mod_name)} was skipped: {title[0].lower() + title[1:]}",
                         explain + f"\n\nSMAPI's reason: {reason}.", steps, acts,
                         [km.id] if km else [], "logs"))
    errs = Counter(m.group(1) for m in re.finditer(r"\[[\d:\s]+ERROR\s+([^\]]+?)\]", text)
                   if m.group(1) not in ("SMAPI", "game", "Content Patcher"))
    for mod_name, n in errs.most_common(5):
        names = names if names is not None else _smapi_names(ctx)
        km = names.get(mod_name.lower())
        out.append(Issue(f"smapi_err:{mod_name}", "warn", f"{_q(mod_name)} reported {n} error(s) last time you played",
                         "The mod loaded but ran into errors while the game was running. That can "
                         "mean a missing or outdated requirement, or a bug in the mod.",
                         ["Update it and the mods it needs.",
                          "If the game misbehaves, turn it off and check whether it stops.",
                          "To see the details, upload SMAPI's log at smapi.io/log."],
                         [Action(f"disable:{km.id}", "Turn it off", auto=False)] if km else [],
                         [km.id] if km else [], "logs"))
    return out


# ====================================================================== redscript (Cyberpunk)
def redscript_issues(ctx: "Ctx") -> list:
    from .doctor import Action, Issue, _q
    f = ctx.inst.game_dir / "r6/logs/redscript_rCURRENT.log"
    if not f.is_file() or time.time() - f.stat().st_mtime > RECENT:
        return []
    text = _read(f)
    bad = Counter()
    for line in text.splitlines():
        if "ERROR" not in line:
            continue
        for path in re.findall(r"r6[\\/]scripts[\\/][^\s:,]+?\.reds", line, re.I):
            bad[path.replace("\\", "/")] += 1
    by_mod: dict = {}
    for path, n in bad.items():
        m = ctx.owner("../" + path) or ctx.owner(path)
        key = m.id if m else path.split("/")[2] if path.count("/") > 2 else path
        by_mod.setdefault(key, [m, 0])[1] += n
    out = []
    for key, (m, n) in by_mod.items():
        name = m.name if m else key
        out.append(Issue(
            f"redscript:{key}", "crash", f"Script mod {_q(name)} fails to compile",
            "Cyberpunk compiles all script mods together when it starts. When one fails, "
            f"redscript reports it and usually none of the script mods load (the game may show a "
            f"warning or crash). redscript's log shows {n} error(s) in this mod. This mostly "
            "happens after a game update, or when a mod needs a newer Codeware/redscript.",
            ["Update the mod and its requirements (Codeware, redscript, ArchiveXL, TweakXL).",
             "If there's no update yet, turn the mod off so the others can load."],
            ([Action(f"update_compatible:{m.id}", "Find the newest version", auto=False)] if m and m.nexus_mod_id else [])
            + ([Action(f"disable:{m.id}", "Turn it off", auto=False)] if m else []),
            [m.id] if m else [], "logs"))
    return out


def issues(ctx: "Ctx") -> list:
    gid = ctx.inst.game.id
    out = []
    if ctx.inst.game.has_plugins:
        out += crash_issues(ctx) + xse_log_issues(ctx)
    if gid == "stardewvalley":
        out += smapi_issues(ctx)
    if gid == "cyberpunk2077":
        out += redscript_issues(ctx)
    return out
