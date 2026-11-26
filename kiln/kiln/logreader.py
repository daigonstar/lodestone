"""Every log a game and its mods wrote, translated into plain English.

The doctor turns the clear-cut problems into Issues; this module is for the Logs
tab, which shows *everything* the last launch wrote: the script extender's own
log (which plugins loaded, how far startup got), its loader, the Engine Fixes
preloader, each SKSE plugin's own log, and crash reports. Many failures are
silent: a plugin logs an error and carries on, or the game dies mid-startup and
the log just stops. Those are what this is for.
"""
from __future__ import annotations

import re
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING

from . import crashlogs

if TYPE_CHECKING:
    from .doctor import Ctx
    from .instance import Instance

LEVELS = ("problem", "warn", "note", "ok")        # most serious first


@dataclass
class Line:
    level: str               # problem | warn | note | ok
    plain: str               # what it means
    raw: str = ""            # the log's own words
    mod: str = ""            # Kiln mod id involved, if known


@dataclass
class Report:
    title: str
    path: Path
    mtime: float
    level: str = "ok"
    summary: str = ""
    lines: list[Line] = field(default_factory=list)
    mods: list[str] = field(default_factory=list)
    last_run: bool = True    # written by the most recent launch

    def add(self, level: str, plain: str, raw: str = "", mod: str = "") -> None:
        self.lines.append(Line(level, plain, raw.strip()[:400], mod))
        if LEVELS.index(level) < LEVELS.index(self.level):
            self.level = level
        if mod and mod not in self.mods:
            self.mods.append(mod)


@dataclass
class Story:
    """What happened the last time the game was started, in a few sentences."""
    level: str
    headline: str
    details: list[str] = field(default_factory=list)
    mods: list[str] = field(default_factory=list)


# ====================================================================== script extender
XSE = {"skyrimse": ("SKSE", "skse64", "SKSE"), "fallout4": ("F4SE", "f4se", "F4SE")}
MESSAGES = {   # SKSE/F4SE message types, in the order a normal start goes through them
    0: "the first start-up signal (plugins setting themselves up)",
    1: "the second start-up signal (plugins connecting to each other)",
    2: "the “about to load a save” signal",
    3: "the “save loaded” signal",
    4: "the “game saved” signal",
    5: "the “save deleted” signal",
    6: "the “controls ready” signal",
    7: "the “new game started” signal",
    8: "the “game data loaded” signal (the main menu is next)",
}
# plugins whose start-up crash has a known cause: dll -> (check(inst) -> reason or "", fix)
def _bdi_reason(inst: "Instance") -> str:
    d = inst.deploy_dir / "SKSE/Plugins/BehaviorDataInjector"
    if not d.is_dir() or not any(d.glob("*.json")):
        return ("It reads its settings from Data\\SKSE\\Plugins\\BehaviorDataInjector\\*.json, and none "
                "of your mods provide any, so it has nothing to do. It's safe to turn off.")
    return ""


KNOWN_STARTUP = {"behaviordatainjector.dll": _bdi_reason}
# frameworks that only act on config files other mods ship: listed as a requirement,
# but not needed when no mod provides a config. Nexus mod id -> reason it's unused ("" = used)
UNUSED_WITHOUT_CONFIG = {78146: _bdi_reason, 78159: _bdi_reason}   # Behavior Data Injector (+ Universal Support)


def unused_requirement(inst: "Instance", nexus_ids) -> str:
    for n in nexus_ids:
        fn = UNUSED_WITHOUT_CONFIG.get(n)
        if fn:
            why = fn(inst)
            if why:
                return why
    return ""


def xse_log(inst: "Instance") -> Path | None:
    x = XSE.get(inst.game.id)
    mg = mygames(inst)
    return mg / x[0] / f"{x[1]}.log" if (x and mg) else None


def mygames(inst: "Instance") -> Path | None:
    return crashlogs._mygames(inst)


def startup_stop(inst: "Instance", text: str | None = None) -> dict | None:
    """If the script extender's log stops in the middle of handing a signal to its
    plugins, the game died inside that plugin: {'dll', 'handle', 'message'}."""
    log = xse_log(inst)
    if text is None:
        if log is None or not log.is_file():
            return None
        text = crashlogs._read(log)
    lines = [ln.strip() for ln in text.splitlines() if ln.strip()]
    if not lines:
        return None
    m = re.match(r"sending message type (\d+) to plugin (\d+)", lines[-1])
    if not m:
        return None
    msg, handle = int(m.group(1)), int(m.group(2))
    dll = ""
    for hm in re.finditer(r"plugin\s+(.+?\.dll)\s+\(.*?\)\s+loaded correctly \(handle (\d+)\)", text):
        if int(hm.group(2)) == handle:
            dll = Path(hm.group(1).replace("\\", "/")).name
    return {"dll": dll, "handle": handle, "message": msg} if dll else None


def _read_xse(ctx: "Ctx", log: Path, running: bool) -> Report:
    inst = ctx.inst
    x = XSE[inst.game.id]
    r = Report(f"{x[2]} (the script extender)", log, log.stat().st_mtime)
    text = crashlogs._read(log)
    pdir = f"{x[0]}/Plugins"

    def named(dll):
        m = ctx.owner(f"{pdir}/{dll}")
        return (f"{dll} (from “{m.name}”)" if m else dll), (m.id if m else "")

    ok = len(re.findall(r"loaded correctly", text))
    rt = re.search(r"runtime: initialize \(version = ([\d.]+) ([0-9A-Fa-f]{8})", text)
    if rt:
        v = int(rt.group(2), 16)
        r.add("ok", f"{x[0]} {rt.group(1)} started on game version "
              f"{v >> 24}.{(v >> 16) & 0xFF}.{(v >> 4) & 0xFFF}.", rt.group(0))
    for m in re.finditer(r"^.*plugin\s+(\S.+?\.dll)\s+\((.*?)\)\s+(.+)$", text, re.M):
        dll = Path(m.group(1).replace("\\", "/")).name
        rest = m.group(3)
        who, mid = named(dll)
        if "loaded correctly" in rest:
            continue
        if "no version data" in rest:
            r.add("problem", f"{who} isn't an {x[0]} plugin {x[0]} can use (it has no version "
                  "information), so it was skipped. It's either built for a much older game or not "
                  "a plugin at all.", m.group(0), mid)
        elif "fatal error occurred while loading" in rest:
            r.add("problem", f"{who} crashed while loading, so {x[0]} turned it off.", m.group(0), mid)
        elif "incompatible" in rest or "disabled" in rest:
            r.add("problem", f"{who} was turned off because it's made for a different game version.",
                  m.group(0), mid)
        else:
            r.add("warn", f"{who}: {rest}", m.group(0), mid)
    for m in re.finditer(r"^.*couldn't load plugin\s+(.+?\.dll)\s*\(Last Error:\s*(\d+)\).*$", text, re.M | re.I):
        dll = Path(m.group(1).replace("\\", "/")).name
        who, mid = named(dll)
        why = crashlogs.LAST_ERRORS.get(m.group(2), f"Windows error {m.group(2)}").format(
            appid=inst.game.steam_appid)
        r.add("problem", f"{who} couldn't be loaded: {why}.", m.group(0), mid)
    tramp = re.findall(r"^.*trampoline alloc .* failed.*$", text, re.M)
    if tramp:
        r.add("note", f"{len(tramp)} “trampoline alloc failed” line(s). Harmless on their own: a plugin "
              "asked for memory at one address and got it at another.", tramp[0])
    stop = startup_stop(inst, text)
    if stop and not running:
        who, mid = named(stop["dll"])
        known = KNOWN_STARTUP.get(stop["dll"].lower())
        extra = known(inst) if known else ""
        r.add("problem", f"The log stops while {x[0]} was handing {MESSAGES.get(stop['message'], 'a signal')} "
              f"to {who}. The game closed at that moment, so that plugin almost certainly crashed it. "
              f"(Plugins don't get to write anything when they crash, which is why nothing says “error”.)"
              + (f" {extra}" if extra else ""),
              f"sending message type {stop['message']} to plugin {stop['handle']}", mid)
        r.summary = f"The game died while starting, inside {who}."
    elif re.search(r"sending message type 8\b", text):
        r.add("ok", "Start-up finished: the game loaded its data and got to the main menu.")
    r.summary = r.summary or (f"{ok} plugin(s) loaded." + (
        f" {sum(1 for ln in r.lines if ln.level == 'problem')} problem(s)." if r.level == "problem" else ""))
    if ok:
        r.lines.insert(0, Line("ok", f"{ok} plugin(s) loaded correctly."))
    return r


# ====================================================================== other logs
ERR = re.compile(r"\[(?:E|error|err|critical|C|fatal)\]|\berror\b|\bfailed\b|\bfailure\b|couldn'?t|"
                 r"could not|unable to|exception|not found|missing", re.I)
WARN = re.compile(r"\[(?:W|warn|warning)\]|\bwarning\b", re.I)
NOT_REALLY = re.compile(r"\b(no errors?|0 errors?|errors?: 0|without errors?|error ?log|"
                        r"failed: 0|0 failed|fix(?:ed)?|installed .* (?:crash|error) fix|patch(?:ed)?|"
                        r"enabled|loaded|\bsuccess)\b", re.I)
_STAMP = re.compile(r"^\s*(?:\[[^\]]*\]\s*)+")          # [time] [thread] [level] prefixes


def _message(raw: str) -> str:
    s = _STAMP.sub("", raw).strip()
    s = re.sub(r"^\S+\.(?:cpp|h|hpp)\s*:\s*\d+\s*\]?\s*:?\s*", "", s)   # source location
    s = s.lstrip(": ").strip()
    return s or raw.strip()


TAG = re.compile(r"\[(E|W|I|D|T|C|error|err|warning|warn|info|debug|trace|critical|fatal)\]", re.I)
TAG_LEVEL = {"e": "problem", "error": "problem", "err": "problem", "c": "problem", "critical": "problem",
             "fatal": "problem", "w": "warn", "warning": "warn", "warn": "warn"}
# lines that look alarming but are normal: (pattern, what to say instead)
HARMLESS = [
    (r"trampoline alloc .* failed", "“Trampoline alloc failed” lines are harmless: the plugin asked for "
     "memory at one address and got it at another."),
    (r"keybinds\.json", "MCM Helper has no saved hotkeys yet. That's normal until you set one in a mod's menu."),
    (r"failed to parse keybind registrations", ""),
]


def _norm(msg: str) -> str:
    return re.sub(r"0x[0-9a-f]+|\b[0-9a-f]{8,}\b|\d+", "#", msg.lower())


def _plugin_log(ctx: "Ctx", p: Path, dlls: dict[str, str]) -> Report:
    stem = p.stem.lower()
    dll = dlls.get(stem) or next((d for s, d in dlls.items() if s.startswith(stem) or stem.startswith(s)), "")
    m = ctx.owner(f"{XSE[ctx.inst.game.id][0]}/Plugins/{dll}") if dll else None
    title = f"{m.name}" if m else p.stem
    r = Report(title, p, p.stat().st_mtime)
    if m:
        r.mods.append(m.id)
    mid = m.id if m else ""
    text = crashlogs._read(p, 1 << 20)
    lines = [ln for ln in text.splitlines() if ln.strip()]
    seen: dict[str, int] = {}
    harmless_said: set[str] = set()
    errs = warns = 0
    for ln in lines:
        msg = _message(ln)
        tag = TAG.search(ln)
        if tag:
            level = TAG_LEVEL.get(tag.group(1).lower(), "")
        elif ERR.search(ln) and not NOT_REALLY.search(msg):
            level = "warn"            # no level tag: it might be an error, it might be chatter
        elif WARN.search(ln):
            level = "warn"
        else:
            level = ""
        if not level:
            continue
        hit = next(((pat, say) for pat, say in HARMLESS if re.search(pat, ln, re.I)), None)
        if hit:
            if hit[1] and hit[0] not in harmless_said:
                harmless_said.add(hit[0])
                r.add("note", hit[1], ln, mid)
            continue
        key = _norm(msg)
        if key in seen:
            seen[key] += 1
            continue
        seen[key] = 1
        if level == "problem":
            errs += 1
            if errs <= 8:
                r.add("problem", _explain(msg, title), ln, mid)
        else:
            warns += 1
            if warns <= 6:
                r.add("warn", _explain(msg, title, warning=True), ln, mid)
    if errs > 8:
        r.add("warn", f"…and {errs - 8} more error(s). Open the file to see them all.")
    repeats = sum(n - 1 for n in seen.values())
    if repeats:
        r.add("note", f"({repeats} repeated line(s) not shown.)")
    if r.level in ("ok", "note"):
        last = _message(lines[-1]) if lines else ""
        r.summary = "Started normally, no errors." + (f" Last thing it wrote: “{last[:120]}”" if last else "")
    else:
        r.summary = ", ".join(x for x in (f"{errs} error(s)" if errs else "",
                                          f"{warns} warning(s)" if warns else "") if x)
    return r


EXPLAIN = [
    (r"\bform\b|formid|\.es[pml]\b|plugin .* not (?:loaded|found)",
     "{who} couldn't find something from a plugin (.esp/.esm): that plugin is probably missing or disabled."),
    (r"(?:file|path|directory|\.\w{2,4}\b).*(?:not found|missing|doesn'?t exist|failed to open)|no such file"
     r"|failed to open|not found",
     "{who} looked for a file that isn't there. Often a missing requirement, or a file a patch expects."),
    (r"version|incompatible|unsupported|runtime",
     "{who} says something doesn't match a version: usually the game version, or a mod it works with."),
    (r"address library|addresslib|offset|id \d+ not found",
     "{who} couldn't find what it needs in Address Library. Update Address Library and this mod."),
    (r"json|parse|syntax|unexpected token",
     "{who} couldn't read one of its settings files (a typo or a broken file from another mod)."),
    (r"hook|trampoline|patch",
     "{who} couldn't hook into the game. Usually it's built for a different game version."),
]


def _explain(msg: str, who: str, warning: bool = False) -> str:
    for pat, text in EXPLAIN:
        if re.search(pat, msg, re.I):
            return (("A warning: " if warning else "") + text.format(who=who)
                    + f" It said: “{msg[:200]}”")
    return f"{who} reported {'a warning' if warning else 'an error'}: “{msg[:200]}”"


def _preloader(ctx: "Ctx", p: Path) -> Report:
    r = Report("Engine Fixes preloader", p, p.stat().st_mtime)
    text = crashlogs._read(p)
    n = len(re.findall(r"loaded successfully", text))
    for m in re.finditer(r'attempting to load "([^"]+)"\s*\n(.*)', text):
        if "success" not in m.group(2).lower():
            dll = m.group(1).replace("\\", "/").rsplit("/", 1)[-1]
            mo = ctx.owner(f"SKSE/Plugins/{dll}")
            r.add("problem", f"It couldn't preload {dll}" + (f" (from “{mo.name}”)" if mo else "")
                  + f": {m.group(2).strip()}", m.group(0), mo.id if mo else "")
    if "loader finished" in text:
        r.add("ok", f"It preloaded {n} plugin(s) and handed over to the game normally.")
    else:
        r.add("warn", "It never got to “loader finished”: the game stopped while it was preloading.")
    r.summary = f"{n} plugin(s) preloaded." if r.level == "ok" else "Problems while preloading."
    return r


def _crash(ctx: "Ctx", p: Path) -> Report:
    r = Report("Crash report", p, p.stat().st_mtime)
    for issue in crashlogs.crash_issues_for(ctx, p):
        for para in [s for s in issue.explain.split("\n") if s.strip()]:
            r.add("problem", para, "", "")
        r.mods += issue.mods
    r.level = "problem"
    r.summary = "The game crashed and your crash logger wrote this report."
    return r


# ====================================================================== all of them
def reports(inst: "Instance", running: bool | None = None) -> list[Report]:
    """Every log Kiln knows how to read for this game, newest first. The ones written
    by the last launch are marked last_run."""
    from .doctor import Ctx
    from . import launchguard
    if running is None:
        running = launchguard.game_running(inst)
    ctx = Ctx(inst)
    out: list[Report] = []
    x = XSE.get(inst.game.id)
    mg = mygames(inst)
    if x and mg:
        main = mg / x[0] / f"{x[1]}.log"
        if main.is_file():
            out.append(_read_xse(ctx, main, running))
        loader = mg / x[0] / f"{x[1]}_loader.log"
        if loader.is_file():
            r = Report(f"{x[0]} loader", loader, loader.stat().st_mtime)
            t = crashlogs._read(loader)
            for ln in t.splitlines():
                if ERR.search(ln) and not NOT_REALLY.search(ln):
                    r.add("problem", f"The loader reported: “{_message(ln)}”", ln)
            if not r.lines:
                r.add("ok", "It started the game normally.")
            r.summary = "Started the game." if r.level == "ok" else "The loader had trouble starting the game."
            out.append(r)
        pdir = inst.deploy_dir / x[0] / "Plugins"
        dlls = {d.stem.lower(): d.name for d in pdir.glob("*.dll")} if pdir.is_dir() else {}
        skip = {f"{x[1]}.log", f"{x[1]}_loader.log", f"{x[1]}_steam_loader.log"}
        logs = [p for p in (mg / x[0]).glob("*.log") if p.name.lower() not in skip and
                not p.name.lower().startswith("crash")]
        logs += [p for p in pdir.glob("*.log")] if pdir.is_dir() else []
        for p in logs:
            out.append(_plugin_log(ctx, p, dlls))
        pre = inst.game_dir / "d3dx9_42.log"
        if pre.is_file():
            out.append(_preloader(ctx, pre))
    for p in crashlogs._crash_files(ctx)[:5]:
        out.append(_crash(ctx, p))
    if not out:
        return out
    # what the last launch wrote: within a couple of minutes of the newest log
    newest = max(r.mtime for r in out)
    for r in out:
        r.last_run = r.mtime >= newest - 180
    out.sort(key=lambda r: (not r.last_run, not r.title.endswith("(the script extender)"),
                            LEVELS.index(r.level), -r.mtime))
    return out


def story(inst: "Instance", reps: list[Report], running: bool = False) -> Story:
    """The last launch, in a few sentences, from the logs it left."""
    game = inst.game.name
    last = [r for r in reps if r.last_run]
    if not last:
        return Story("note", f"No logs yet. Start {game} from Kiln once, then come back here.")
    when = time.strftime("%H:%M", time.localtime(max(r.mtime for r in last)))
    xse = next((r for r in last if r.title.endswith("(the script extender)")), None)
    crash = next((r for r in last if r.title == "Crash report"), None)
    stopped = xse and any("The log stops while" in ln.plain for ln in xse.lines)
    details, mods = [], []
    if running:
        head, level = f"{game} is running now (logs from {when}).", "note"
    elif crash:
        head, level = f"{game} crashed during your last launch ({when}).", "problem"
        details.append(crash.lines[0].plain if crash.lines else "")
        mods += crash.mods
    elif stopped:
        ln = next(ln for ln in xse.lines if "The log stops while" in ln.plain)
        head, level = f"{game} closed by itself while starting up ({when}).", "problem"
        details.append(ln.plain)
        if ln.mod:
            mods.append(ln.mod)
    elif xse and any("Start-up finished" in ln.plain for ln in xse.lines):
        head, level = f"Your last launch ({when}) got to the main menu.", "ok"
    else:
        head, level = f"Your last launch was at {when}.", "note"
    took = startup_time(inst, last)
    if took is not None and not running:
        m, sec = divmod(round(took), 60)
        details.insert(0, f"Start-up took {m} min {sec:02d} s to reach the main menu." if m else
                       f"Start-up took {sec} s to reach the main menu.")
    bad = [r for r in last if r.level == "problem" and r is not xse and r is not crash]
    if bad:
        details.append(f"{len(bad)} mod(s) reported errors in their own logs: "
                       + ", ".join(r.title for r in bad[:5]) + ("…" if len(bad) > 5 else "") + ".")
    if xse and not crash and inst.game.id in XSE and not _has_crash_logger(inst) and level == "problem":
        details.append("No crash report was written because you don't have a crash logger installed. "
                       "Crash Logger SSE adds one, so next time Kiln can say exactly what crashed.")
    return Story(level, head, [d for d in details if d], mods)


def startup_time(inst: "Instance", reps: list[Report]) -> float | None:
    """Seconds from start to the main menu, as Engine Fixes measured it last launch."""
    for r in sorted(reps, key=lambda r: -r.mtime):
        if r.path.name.lower() == "enginefixes.log":
            hits = re.findall(r"time to main menu\s+(\d+)", crashlogs._read(r.path))
            if hits:
                return int(hits[-1]) / 1000
    return None


def _has_crash_logger(inst: "Instance") -> bool:
    """Crash Logger SSE or Trainwreck. (.NET Script Framework only counts on 1.5.97, the
    only version it runs on; a leftover NetScriptFramework folder doesn't.)"""
    pdir = inst.deploy_dir / "SKSE/Plugins"
    if any((pdir / n).is_file() for n in ("CrashLogger.dll", "Trainwreck.dll")):
        return True
    from . import essentials
    rt = essentials.game_runtime(inst)
    return bool(rt and tuple(rt[:3]) <= (1, 5, 97)) and (pdir / "NetScriptFramework.x64.dll").is_file()
