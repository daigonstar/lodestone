"""Mod Doctor: find what's wrong with a mod setup, explain it in plain English,
and fix what can be fixed safely.

diagnose(inst) runs every check and returns Issues, worst first. Each issue
says what's wrong, why it matters, and what to do, and can carry actions:
data fixes that apply() performs here (turning mods/plugins on or off, fixing
the order, moving misplaced files), and fixes the window carries out
(deploying, downloading, opening pages).
"""
from __future__ import annotations

import os
import re
import shutil
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING

from . import deploy

if TYPE_CHECKING:
    from .instance import Instance, Mod

SEVERITY = {          # order, label shown to the user
    "crash": (0, "Will crash the game"),
    "broken": (1, "Something won't work"),
    "warn": (2, "Might cause problems"),
    "info": (3, "Good to know"),
}


@dataclass
class Action:
    fix: str                  # see apply() and the window's run_doctor_action()
    label: str
    auto: bool = True         # part of “Fix everything”
    describe: str = ""        # what “Fix everything” will say it does
    data: dict = field(default_factory=dict)


@dataclass
class Issue:
    key: str                  # stable id (used to hide issues the user dismissed)
    severity: str
    title: str
    explain: str              # what it means and why it matters
    steps: list[str] = field(default_factory=list)   # how to fix it by hand
    actions: list[Action] = field(default_factory=list)
    mods: list[str] = field(default_factory=list)    # mod ids involved
    source: str = ""          # which check found it

    @property
    def rank(self) -> int:
        return SEVERITY[self.severity][0]

    @property
    def auto_action(self) -> Action | None:
        return next((a for a in self.actions if a.auto), None)


# ====================================================================== context
class Ctx:
    """Things several checks need, worked out once."""

    def __init__(self, inst: "Instance"):
        self.inst = inst
        self.desired, self.conflicts = deploy.plan(inst)       # {path_key: (mod id, rel, src)}
        self.by_id = {m.id: m for m in inst.mods}
        self.superseded: set[str] = set()      # old duplicate copies (their own issue covers them)

    def owner(self, game_rel: str) -> "Mod | None":
        """The mod whose file ends up at this deploy-relative path."""
        hit = self.desired.get(game_rel.lower())
        return self.by_id.get(hit[0]) if hit else None

    def name(self, mod_id: str) -> str:
        m = self.by_id.get(mod_id)
        return m.name if m else mod_id


def _q(s: str) -> str:
    return f"“{s}”"


def _list(names, n=4) -> str:
    names = list(names)
    s = ", ".join(names[:n])
    return s + (f" and {len(names) - n} more" if len(names) > n else "")


# ====================================================================== checks
def check_setup(ctx: Ctx) -> list[Issue]:
    """Deployment, other managers' links, mod loaders, Steam launch options, missing
    Stardew requirements: the launch checks, explained."""
    from . import launchguard
    inst = ctx.inst
    out = []
    for c in launchguard.preflight(inst):
        if c.level == "ok" or c.fix == "plugins":       # plugins get their own, richer check
            continue
        if c.fix == "deploy":
            out.append(Issue(
                "deploy", "broken", "The game folder doesn't match your mod list",
                "Kiln keeps your mods in its own folder and places them into the game when it "
                "deploys. Right now some changes haven't been placed yet, or something (usually a "
                "Steam update or “Verify integrity of game files”) removed files, so the game "
                "won't see your mods as they are in Kiln. " + (c.detail or ""),
                ["Click Fix. Kiln re-deploys your mods; it takes a few seconds."],
                [Action("deploy", "Deploy now", describe="Deploy your mods into the game folder")],
                source="deploy"))
        elif c.fix == "materialize":
            out.append(Issue(
                "foreign_links", "warn", "Some game files belong to another mod manager",
                "Files in the game folder are links into another mod manager's folder (for example "
                "Vortex or Amethyst). If that manager is removed or cleans up, those files vanish "
                "and the game breaks without warning.",
                ["Click Fix to turn them into real files owned by the game folder.",
                 "Or remove the mods from the other manager and install them in Kiln."],
                [Action("materialize", "Turn into real files", auto=False, data=c.data)], source="deploy"))
        elif c.fix.startswith("loader:"):
            nf = c.data.get("nexus_file") or [0, ""]
            acts = ([Action(f"install_named:{nf[0]}:{nf[1]}", "Install it", auto=False)] if nf[0] else []) + \
                [Action(c.fix, "Open its page" if nf[0] else "Get it", auto=False, data=c.data)]
            out.append(Issue(
                c.fix, "crash" if nf[0] else "broken", c.title,
                c.data.get("why") or (
                    "This game needs a mod loader (a small program that starts first and loads the "
                    "mods). Without it, the mods that need it are simply ignored: the game starts "
                    "but looks unmodded, or other mods that rely on them crash."),
                ["Click “Install it” and Kiln downloads it from Nexus and puts it in the game folder."
                 if nf[0] else "Click Fix and Kiln gets the loader for you.",
                 f"Or download it yourself from {c.data.get('url', 'its website')} and install it in Kiln."],
                acts, source="loaders"))
        elif c.fix == "launch_options":
            out.append(Issue(
                "launch_options", "broken", "Steam starts the game without the mod loader",
                "Mod loaders only run if Steam starts the game through them. This game's Steam "
                "launch options don't do that yet, so pressing Play in Steam would start it "
                f"unmodded.\n\nNow: {c.data.get('current') or '(empty)'}\nNeeded: {c.data.get('wanted')}",
                ["Click Fix. Kiln sets the launch options (Steam has to be closed for a moment).",
                 "Or paste the “Needed” line into Steam → right-click the game → Properties → "
                 "Launch options."],
                [Action("launch_options", "Fix launch options", auto=False, data=c.data,
                        describe="Set Steam's launch options")], source="loaders"))
        elif c.fix == "deps":
            out.append(Issue(
                "deps", "broken", c.title,
                "Some mods need other mods to work, and those aren't installed. The game's mod "
                "loader skips them, so they do nothing.\n\n" + c.detail,
                ["Click Fix to see the missing mods and install them."],
                [Action("deps", "Install missing…", auto=False)], source="deps"))
        else:
            out.append(Issue(c.fix or c.title, "warn", c.title, c.detail, source="setup"))
    return out


def check_empty(ctx: Ctx) -> list[Issue]:
    """Mods that installed no files at all."""
    out = []
    for m in ctx.inst.mods:
        if not m.enabled:
            continue
        root = ctx.inst.mod_path(m)
        has = root.is_dir() and bool(ctx.inst.mod_files(m))
        if has:
            continue
        arc = ctx.inst.downloads_dir / m.archive if m.archive else None
        acts = [Action(f"reinstall:{m.id}", "Reinstall", auto=False)] if arc and arc.is_file() else []
        acts.append(Action(f"disable:{m.id}", "Turn it off", auto=False))
        out.append(Issue(
            f"empty:{m.id}", "broken", f"{_q(m.name)} installed no files",
            "The mod is in your list but nothing from it reaches the game. Usually the download "
            "was interrupted, or the mod has an installer with choices and none were picked.",
            ["Reinstall it from the download (Kiln shows its installer if it has one).",
             "If it still installs nothing, open its Nexus page: the file you got may be the "
             "wrong one (a readme, or a version for another game edition)."],
            acts, [m.id], "files"))
    return out


def check_misplaced(ctx: Ctx) -> list[Issue]:
    """Bethesda mods whose files sit one folder too deep (e.g. ModName/meshes/…)."""
    inst = ctx.inst
    if not inst.game.has_plugins:
        return []
    from .games.bethesda import DATA_EXTS, DATA_MARKERS
    out = []
    for m in inst.mods:
        root = inst.mod_path(m)
        if not m.enabled or not root.is_dir():
            continue
        top = [c for c in root.iterdir() if not c.name.startswith(".")]
        if any(c.name.lower() in DATA_MARKERS or c.suffix.lower() in DATA_EXTS
               or c.name == "__root__" for c in top):
            continue
        dirs = [c for c in top if c.is_dir()]
        if len(dirs) != 1:
            continue
        inner = dirs[0]
        if inner.name.lower() == "data":
            inner_ok = True
        else:
            kids = list(inner.iterdir())
            inner_ok = any(k.name.lower() in DATA_MARKERS or k.suffix.lower() in DATA_EXTS for k in kids)
        if inner_ok:
            out.append(Issue(
                f"misplaced:{m.id}", "broken", f"{_q(m.name)} is installed one folder too deep",
                f"Its files are inside an extra folder ({inner.name}/), so they'd end up in "
                f"Data/{inner.name}/… where the game never looks. The mod does nothing.",
                ["Click Fix and Kiln moves the files up one level."],
                [Action(f"relayout:{m.id}", "Move files up",
                        describe=f"Move {m.name}'s files out of the extra {inner.name}/ folder")],
                [m.id], "files"))
    return out


def _file_id(m) -> int:
    return int(m.nexus_file_id or 0)


def check_duplicates(ctx: Ctx) -> list[Issue]:
    """The same Nexus mod installed more than once (often by two collections), where
    the copies overlap: only one copy's files can win, and it may be the old one."""
    inst = ctx.inst
    groups: dict[int, list] = {}
    for m in inst.mods:
        if m.enabled and m.nexus_mod_id:
            groups.setdefault(m.nexus_mod_id, []).append(m)
    files: dict[str, set[str]] = {}
    for mod, rel, _p in deploy.staged_files(inst):
        files.setdefault(mod.id, set()).add(rel.lower())
    out = []
    for nid, ms in groups.items():
        if len(ms) < 2:
            continue
        # keep the copy that works on this game version; among those, the newest
        # (Nexus file ids grow over time)
        ms = sorted(ms, key=lambda m: (_fits_game(ctx, m), _file_id(m)))
        newest = ms[-1]
        nf = files.get(newest.id, set())
        old = []
        for m in ms[:-1]:
            mf = files.get(m.id, set())
            shared = mf & nf
            # a copy is a duplicate only when the newer one covers (nearly) all of it.
            # A main file plus its update / patch / add-on from the same page shares a
            # few files, but the main file still holds the rest (the .esp, the meshes),
            # and the patch needs it.
            if shared and len(shared) >= 0.8 * len(mf) and len(mf) >= 0.5 * len(nf):
                old.append((m, shared))
        if not old:
            continue
        losing = [m for m, shared in old
                  if any(ctx.desired.get(k, ("",))[0] == m.id for k in shared)]
        sev = "crash" if any(k.endswith(".dll") for _m, sh in old for k in sh) and losing else \
            ("broken" if losing else "warn")
        names = [_label(m) for m, _ in old]
        ctx.superseded |= {m.id for m, _ in old}
        how = (f"Right now the older copy's files are the ones the game uses ({_list(_label(m) for m in losing)})."
               if losing else "The newest copy wins at the moment, but the old one is just clutter "
                              "that can win again if the order changes.")
        out.append(Issue(
            f"dupe:{nid}", sev, f"{_q(newest.name)} is installed {len(old) + 1} times",
            f"Different copies of the same mod are installed: {_list(names + [_label(newest)])}. "
            "This usually happens when two collections each bring their own version. Both "
            "copies contain the same files, so only one can be used. " + how +
            " Old versions of SKSE plugins in particular are a common cause of crashes after a "
            "game update.",
            [f"Keep the newest copy ({_label(newest)}) and turn the "
             "others off. Click Fix to do that.",
             "If you know you need the older one (rare), turn the newer one off instead."],
            [Action("disable:" + ",".join(m.id for m, _ in old), "Keep the newest, turn off the rest",
                    describe=f"Turn off the older copies of {newest.name}")],
            [m.id for m in ms], "duplicates"))
    return out


PATCHY = re.compile(r"\b(update|patch|hotfix|fix|add-?on|requires? main|main file needed|no startup)\b", re.I)
CONTENT = (".esp", ".esm", ".esl", ".bsa")


def check_main_file_off(ctx: Ctx) -> list[Issue]:
    """An update / patch / add-on of a mod is on, but the mod's main file (from the
    same Nexus page) is off: the patch replaces a few files and needs the rest."""
    inst = ctx.inst
    by_nexus: dict[int, list] = {}
    for m in inst.mods:
        if m.nexus_mod_id:
            by_nexus.setdefault(m.nexus_mod_id, []).append(m)
    files: dict[str, set[str]] = {}
    for ms in by_nexus.values():
        if any(m.enabled for m in ms) and not all(m.enabled for m in ms):
            for m in ms:            # switched-off mods too (staged_files only lists enabled ones)
                files[m.id] = {deploy.deploy_rel(inst, r).lower() for r in inst.mod_files(m)}
    out, done = [], set()
    for ms in by_nexus.values():
        on = [m for m in ms if m.enabled]
        if not on or len(on) == len(ms):
            continue
        for patch in on:
            pf = files.get(patch.id, set())
            best = None
            for main in ms:
                if main.enabled or main.id in done or main.id in ctx.superseded:
                    continue
                mf = files.get(main.id, set())
                if not (pf & mf) or len(mf) < 3 * len(pf):
                    continue                                 # not its main file
                extra_content = any(f.endswith(CONTENT) for f in mf - pf)
                if not (extra_content or PATCHY.search(patch.file_name or patch.name)):
                    continue                                 # a smaller alternative, not a patch
                if any(o.enabled and len(files.get(o.id, set()) & mf) >= 0.8 * len(mf) for o in ms if o is not main):
                    continue                                 # another copy of the main file is on
                if best is None or _file_id(main) > _file_id(best):
                    best = main
            if best is None:
                continue
            done.add(best.id)
            out.append(Issue(
                f"main_off:{best.id}", "broken",
                f"{_q(patch.name)} needs its main file, which is off",
                f"{_label(patch)} is an update or add-on for {_label(best)}: it replaces "
                f"{len(pf & files.get(best.id, set()))} of the main file's {len(files.get(best.id, set()))} files "
                "and needs the rest. The main file is turned off, so the mod is missing its plugin and "
                "most of its content. Menus that set the mod up (MCM) wait for it and never find it.",
                [f"Turn on {_q(best.name)} and place it just before {_q(patch.name)}, so the update's "
                 "files still win. Click Fix to do that."],
                [Action(f"enable_main:{best.id}|{patch.id}", f"Turn on {best.name}",
                        describe=f"Turn on {best.name} (the main file {patch.name} needs)")],
                [best.id, patch.id], "duplicates"))
    return out


def _fits_game(ctx: Ctx, m) -> bool:
    """False if one of the mod's SKSE plugins won't load on the installed game version."""
    cache = ctx.__dict__.setdefault("_fits", {})
    if m.id not in cache:
        from . import pe, versionmatch
        rt = runtime(ctx.inst) if ctx.inst.game.id in XSE else None
        ok = True
        if rt:
            for f in versionmatch._staged_dlls(ctx.inst, m):
                if pe.xse_plugin(f).verdict(rt, None, True)[0] not in ("ok", "addrlib"):
                    ok = False
                    break
        cache[m.id] = ok
    return cache[m.id]


def working_copy(ctx: Ctx, m):
    """Another downloaded copy of the same Nexus mod (switched off) whose SKSE plugins
    load on this game version."""
    if not m or not m.nexus_mod_id:
        return None
    from . import versionmatch
    cands = [o for o in ctx.inst.mods if o is not m and o.nexus_mod_id == m.nexus_mod_id and not o.enabled
             and versionmatch._staged_dlls(ctx.inst, o)]
    good = [o for o in cands if _fits_game(ctx, o)]
    return max(good, key=_file_id) if good else None


def _label(m) -> str:
    extra = m.file_name if m.file_name and m.file_name.lower() not in m.name.lower() else ""
    bits = [b for b in (extra, f"v{m.version}" if m.version else "") if b]
    return f"{m.name} ({', '.join(bits)})" if bits else m.name


def check_dll_clash(ctx: Ctx) -> list[Issue]:
    """Two different mods shipping the same SKSE/F4SE plugin (an old mod and its 'NG'
    remake, say): keep the newest build."""
    inst = ctx.inst
    if not inst.game.has_plugins:
        return []
    from . import pe
    dupe_ids = ctx.superseded            # filled by check_duplicates, which runs first
    out, seen = [], set()
    for key, ids in ctx.conflicts.items():
        if not key.endswith(".dll") or "/plugins/" not in key:
            continue
        ids = list(dict.fromkeys(ids))
        mods = [ctx.by_id[i] for i in ids if i in ctx.by_id]
        if len(mods) < 2 or len({m.nexus_mod_id for m in mods}) == 1 or set(ids) <= dupe_ids:
            continue
        builds = {}
        for m in mods:
            src = _find_staged(inst, m, key)
            builds[m.id] = pe.xse_plugin(src).build_time if src else 0
        newest = max(mods, key=lambda m: builds[m.id])
        winner = ctx.desired[key][0]
        pair = tuple(sorted(ids))
        if pair in seen:
            continue
        seen.add(pair)
        dll = key.rsplit("/", 1)[-1]
        if winner == newest.id:
            others = [m for m in mods if m.id != newest.id]
            out.append(Issue(
                f"dllclash:{dll}", "warn", f"{_list(m.name for m in mods)} contain the same plugin ({dll})",
                f"These look like two versions of the same mod (often an original and a newer "
                f"remake). Kiln uses the newest build, from {_q(newest.name)}, so this is fine, "
                f"but {_list(m.name for m in others)} is probably not needed any more.",
                [f"Turn off {_list(m.name for m in others)} if it has nothing else you use."],
                [Action("disable:" + ",".join(m.id for m in others), "Turn off the older one", auto=False)],
                list(ids), "duplicates"))
        else:
            out.append(Issue(
                f"dllclash:{dll}", "broken",
                f"An older copy of {dll} wins over the newer one",
                f"{_q(ctx.name(winner))} and {_q(newest.name)} both contain {dll}. The game uses "
                f"the one from {_q(ctx.name(winner))}, which is an older build. Old SKSE plugins "
                "often don't work with the current game and can crash it.",
                [f"Click Fix to let {_q(newest.name)} win (Kiln remembers the choice).",
                 f"Or turn {_q(ctx.name(winner))} off if it has nothing else you use."],
                [Action(f"win:{newest.id}:{winner}", f"Use {newest.name}'s copy",
                        describe=f"Let {newest.name} win over {ctx.name(winner)} for {dll}")],
                list(ids), "duplicates"))
    return out


def _find_staged(inst, m, key) -> Path | None:
    root = inst.mod_path(m)
    for rel in inst.mod_files(m):
        if deploy.deploy_rel(inst, rel).lower() == key:
            return root / rel
    return None


# ---------------------------------------------------------------- script extender
XSE = {
    "skyrimse": {"name": "SKSE", "exe": "SkyrimSE.exe", "loader": "skse64_loader.exe",
                 "dll": "skse64_{}.dll", "plugins": "SKSE/Plugins", "nexus": 30379, "game": "Skyrim",
                 "addrlib": 32444, "url": "https://skse.silverlock.org/"},
}


def runtime(inst) -> tuple[int, int, int] | None:
    from . import pe
    x = XSE.get(inst.game.id)
    v = pe.file_version(inst.game_dir / x["exe"]) if x else None
    return tuple(v[:3]) if v else None


def check_script_extender(ctx: Ctx) -> list[Issue]:
    inst = ctx.inst
    x = XSE.get(inst.game.id)
    if not x:
        return []
    from . import pe
    from .archive import ci_resolve
    rt = runtime(inst)
    if not rt:
        return []
    rts = ".".join(map(str, rt))
    out = []
    loader = ci_resolve(inst.game_dir, x["loader"])
    dll_name = x["dll"].format("_".join(map(str, rt)))
    xse_dll = ci_resolve(inst.game_dir, dll_name)
    xse_ver = pe.xse_version(Path(xse_dll)) if xse_dll else (pe.xse_version(Path(loader)) if loader else None)
    plugins_dir = ci_resolve(inst.game_dir / inst.game.deploy_root, x["plugins"])
    uses = plugins_dir and any(Path(plugins_dir).glob("*.dll"))
    if loader and not xse_dll:
        have = sorted(p.name for p in inst.game_dir.glob(x["dll"].format("*")))
        made = have[0].split("_", 1)[1].rsplit(".", 1)[0].replace("_", ".") if have else "another version"
        out.append(Issue(
            "xse_version", "crash", f"{x['name']} doesn't match your {x['game']} version",
            f"Your game is version {rts}, but the {x['name']} you have is for {made}. This "
            f"happens when Steam updates {x['game']}. {x['name']} refuses to start, so every "
            f"mod that needs it stops working (and some crash the game).",
            [f"Click “Install {x['name']} for {rts}”: Kiln finds that build among the {x['name']} "
             "files on Nexus (older builds are kept there) and installs it.",
             f"Or download it from {x['url']} and drag the archive onto the Mods page.",
             f"Or, if the new {x['name']} isn't out yet, wait for it: they usually follow a "
             "game update within days. Many people stop Steam from updating the game by "
             "launching through Kiln only after checking."],
            [Action("match_version", f"Install {x['name']} for {rts}", auto=False),
             Action(f"url:{x['url']}", f"{x['name']} website", auto=False)], source="xse"))
    if not uses or not xse_dll:
        return out
    # Address Library for this exact game version
    al_files = [f"versionlib-{rt[0]}-{rt[1]}-{rt[2]}-0.bin", f"version-{rt[0]}-{rt[1]}-{rt[2]}-0.bin"]
    addrlib = any(ci_resolve(Path(plugins_dir), f) for f in al_files)
    al_mod = inst.find_nexus(x["addrlib"])
    if not addrlib:
        out.append(Issue(
            "addrlib", "broken", f"Address Library doesn't cover {x['game']} {rts}",
            "Address Library is a lookup table most SKSE plugins use to find their way around "
            f"the game. Yours has no table for {rts}, so {x['name']} switches off every plugin "
            "that relies on it.",
            ["Update Address Library to the newest version (Click Fix).",
             "If there's no table for your version yet, it'll come soon after the game update."],
            [Action(f"update_compatible:{al_mod.id}" if al_mod else f"nexus_page:{x['addrlib']}",
                    "Update Address Library", auto=False)], [al_mod.id] if al_mod else [], "xse"))
    # each plugin, by SKSE's own rules
    by_mod: dict[str, list[tuple[str, str, str]]] = {}
    sig_old: dict[str, list[str]] = {}
    try:
        exe_built = pe._headers((inst.game_dir / x["exe"]).read_bytes())["timestamp"]
    except (OSError, pe.PEError):
        exe_built = 0
    for f in sorted(Path(plugins_dir).glob("*.dll"), key=lambda p: p.name.lower()):
        info = pe.xse_plugin(f)
        code, why = info.verdict(rt, xse_ver, addrlib)
        if code == "ok" and info.kind == "plugin" and info.independence & pe.SIGNATURES \
                and not info.independence & pe.ADDRLIB and exe_built \
                and 520128000 < info.build_time < exe_built - 86400:
            m = ctx.owner(f"{x['plugins']}/{f.name}")
            if not (m and m.id in ctx.superseded):
                sig_old.setdefault(m.id if m else "", []).append(f.name)
            continue
        if code in ("ok", "addrlib"):
            continue
        rel = f"{x['plugins']}/{f.name}"
        m = ctx.owner(rel)
        if m and m.id in ctx.superseded:
            continue                       # an old duplicate: “installed twice” covers it
        by_mod.setdefault(m.id if m else "", []).append((f.name, code, why))
    for mid, names in sig_old.items():
        m = ctx.by_id.get(mid)
        files = ", ".join(names)
        acts = ([Action(f"update_compatible:{m.id}", "Find a version that works", auto=False)]
                if m and m.nexus_mod_id else []) + ([Action(f"disable:{m.id}", "Turn it off", auto=False)] if m else [])
        out.append(Issue(
            f"xse_pattern:{mid or files}", "warn",
            f"{_q(m.name) if m else files} may not work on {x['game']} {rts}",
            f"{files} doesn't use Address Library: it finds the game code it changes by searching "
            f"for byte patterns. It was built before your version of {x['game']} existed, and "
            f"when the game's code changed the pattern isn't found. That's what an “[ERROR] "
            f"Unknown pattern: …” box at start-up means, and the mod then does nothing (or the "
            "game closes).",
            ["If you've seen an “Unknown pattern” error, update this mod or turn it off.",
             "Otherwise it may still work: test it in game."],
            acts, [mid] if m else [], "xse"))
    if len([k for k in by_mod if k]) >= 2:
        out.append(Issue(
            "xse_match_all", "broken", f"{len(by_mod)} mods don't fit {x['game']} {rts}",
            f"These mods have SKSE plugins made for other game versions. Kiln can look up the "
            f"file made for {rts} on each mod's Nexus page (older files included) and install them "
            "in one go.",
            ["Click “Match mods to this version”, check what Kiln picked, and install.",
             "Or fix them one at a time below."],
            [Action("match_version", "Match mods to this version", auto=False)],
            [k for k in by_mod if k], "xse"))
    for mid, bad in by_mod.items():
        m = ctx.by_id.get(mid)
        files = ", ".join(n for n, _c, _w in bad)
        codes = {c for _n, c, _w in bad}
        if "old_xse" in codes and len(codes) == 1:
            out.append(Issue(
                f"xse_old:{mid}", "broken",
                f"{_q(m.name) if m else files} needs a newer {x['name']}",
                f"{files}: {bad[0][2]}. You have {x['name']} {'.'.join(map(str, xse_ver or ()))}, so "
                f"{x['name']} won't load it.",
                [f"Update {x['name']} (Click Fix to open its page)."],
                [Action(f"nexus_page:{x['nexus']}", f"Get {x['name']}", auto=False)],
                [mid] if m else [], "xse"))
            continue
        why = "; ".join(sorted({w for _n, c, w in bad if c != "old_xse"}))
        acts = []
        alt = working_copy(ctx, m) if m else None
        if alt is not None:     # a copy that works is already downloaded: just switch over
            acts.append(Action(f"swap:{alt.id}:{m.id}", f"Use the {alt.version or 'other'} copy you have",
                               describe=f"Switch {m.name} to the copy made for {rts} ({_label(alt)})"))
        if m and m.nexus_mod_id:
            acts.append(Action(f"update_compatible:{m.id}", "Find a version that works", auto=False))
        if m:
            acts.append(Action(f"disable:{m.id}", "Turn it off", auto=False))
        out.append(Issue(
            f"xse_incompatible:{mid or files}", "broken",
            (f"{_q(m.name)} won't load on {x['game']} {rts}" if m else
             f"{files} won't load on {x['game']} {rts}"),
            f"{x['name']} plugins are built for particular {x['game']} versions. "
            f"{'This mod' if m else 'This file'} ({files}) was {why}. {x['name']} skips it, so "
            "it silently does nothing, and mods that build on it can misbehave or crash."
            + ("" if m else " Kiln didn't install it, so it was put in the game folder by hand or "
                            "by another tool."),
            ["Get the newest version of the mod from its Nexus page. Look for a file that "
             f"mentions {rts}, “{rt[0]}.{rt[1]}” or “latest”; Click “Find a version that works” "
             "and Kiln looks for you.",
             "If there's no version for your game yet, turn the mod off for now.",
             "If it came from a collection, that collection was made for an older game version: "
             "other plugins in it may have the same problem."],
            acts, [mid] if m else [], "xse"))
    return out


# ---------------------------------------------------------------- plugins (esp/esm/esl)
def check_plugins(ctx: Ctx) -> list[Issue]:
    inst = ctx.inst
    if not inst.game.has_plugins or not inst.plugins:
        return []
    from .games.bethesda import read_plugin
    out = []
    implicit = [n for n in inst.game.implicit_plugins(inst.game_dir)]
    implicit_low = {n.lower() for n in implicit}
    info = {p["name"].lower(): read_plugin(inst.deploy_dir / p["name"]) for p in inst.plugins}
    enabled = [p["name"] for p in inst.plugins if p["enabled"]]
    pos = {n.lower(): i for i, n in enumerate(enabled)}
    present_off = {p["name"].lower(): p["name"] for p in inst.plugins if not p["enabled"]}
    # masters
    disabled_mod_plugins = {}
    for m in inst.mods:
        if not m.enabled and inst.mod_path(m).is_dir():
            for c in inst.mod_path(m).iterdir():
                if c.suffix.lower() in (".esp", ".esm", ".esl"):
                    disabled_mod_plugins.setdefault(c.name.lower(), m)
    need_sort = []
    for name in enabled:
        pi = info.get(name.lower())
        if not pi:
            continue
        owner = ctx.owner(name)
        for mst in pi.masters:
            low = mst.lower()
            if low in implicit_low:
                continue
            if low in pos:
                if pos[low] > pos[name.lower()]:
                    need_sort.append((name, mst))
                continue
            if low in present_off:
                out.append(Issue(
                    f"master_off:{name}:{mst}", "crash", f"{name} needs {mst}, which is switched off",
                    f"{name} is built on top of {mst} (its “master”). {mst} is installed but "
                    "turned off in your plugin list, and the game crashes on start-up when a "
                    "plugin's master is missing.",
                    [f"Turn {mst} on (Click Fix)."],
                    [Action(f"enable_plugin:{present_off[low]}", f"Turn {mst} on",
                            describe=f"Turn on {mst} (needed by {name})")],
                    [owner.id] if owner else [], "plugins"))
            elif low in disabled_mod_plugins:
                dm = disabled_mod_plugins[low]
                out.append(Issue(
                    f"master_mod_off:{name}:{mst}", "crash",
                    f"{name} needs {mst}, from the mod {_q(dm.name)} which is turned off",
                    f"{name} is built on top of {mst}, which comes with {_q(dm.name)}. That mod is "
                    "turned off, so the game will crash on start-up.",
                    [f"Turn {_q(dm.name)} on (Click Fix)."],
                    [Action(f"enable_mod:{dm.id}", f"Turn {dm.name} on",
                            describe=f"Turn on {dm.name} (needed by {name})")],
                    [dm.id] + ([owner.id] if owner else []), "plugins"))
            else:
                out.append(Issue(
                    f"master_missing:{name}:{mst}", "crash", f"{name} needs {mst}, which you don't have",
                    f"{name}" + (f" (from {_q(owner.name)})" if owner else "") + f" is built on top "
                    f"of {mst} (its “master”), which isn't installed. Skyrim and Fallout crash "
                    "right at start-up when a master is missing (the classic “crash before the "
                    "main menu”).",
                    [f"Install the mod that contains {mst}: its name is usually close to the file "
                     "name. Click “Find it on Nexus” to search.",
                     f"Or turn off {name} if you don't need it. Click Fix to do that so the game "
                     "starts; you can turn it back on after installing the master."],
                    [Action(f"search:{Path(mst).stem}", "Find it on Nexus", auto=False),
                     Action(f"disable_plugin:{name}", f"Turn {name} off",
                            describe=f"Turn off {name} (its master {mst} isn't installed)")],
                    [owner.id] if owner else [], "plugins"))
    if need_sort:
        out.append(Issue(
            "plugin_order", "crash", f"{len(need_sort)} plugin(s) load before the files they need",
            "A plugin has to load after its masters. These load too early: "
            + _list(f"{a} before {b}" for a, b in need_sort) + ". This crashes the game or makes "
            "things disappear.",
            ["Click Fix and Kiln sorts your plugins (masters first, then by what needs what)."],
            [Action("sort_plugins", "Sort plugins", describe="Sort the plugin load order")],
            [], "plugins"))
    full = [n for n in enabled if not (info.get(n.lower()) and info[n.lower()].is_light)]
    n_full = len(full) + len(implicit)
    light = len(enabled) - len(full)
    if n_full > 254:
        out.append(Issue(
            "plugin_limit", "crash", f"Too many plugins: {n_full} of 254",
            "The game can load at most 254 regular plugins (light “ESL” plugins don't count). "
            f"You have {n_full} including the base game's, so the game crashes or loses saves.",
            [f"Turn off {n_full - 254} or more plugins you can live without.",
             "Advanced: many small plugins can be made light (ESL-flagged) with SSEEdit; "
             "big modlists do this."],
            [Action("plugins", "Show plugins", auto=False)], [], "plugins"))
    elif n_full > 240:
        out.append(Issue("plugin_limit", "warn", f"Nearly at the plugin limit ({n_full} of 254)",
                         "The game can't load more than 254 regular plugins. A few more and it "
                         "will crash.", [], [], [], "plugins"))
    if light > 4096:
        out.append(Issue("light_limit", "crash", f"Too many light plugins ({light} of 4096)",
                         "The game can load at most 4096 light plugins.", [], [], [], "plugins"))
    return out


# ---------------------------------------------------------------- known tools
KNOWN = {
    # nexus id: (severity, title, explanation, steps)
    3038: ("warn", "FNIS needs its generator run after every change",
           "FNIS (Fores New Idles) only works after you run “GenerateFNISforUsers” whenever "
           "animation mods change. If it hasn't been run, characters can stand in a T-pose or "
           "animations break. It's an old tool; most modern setups use Pandora or Nemesis "
           "instead, and mixing them causes problems.",
           ["Run FNIS's generator: in the mod's folder, Data/tools/GenerateFNIS_for_Users/"
            "GenerateFNISforUsers.exe, through Proton (for example with protontricks-launch).",
            "Or replace FNIS with Pandora Behaviour Engine+, which is easier to run on Linux."]),
    33746: ("warn", "Dynamic Animation Replacer has been replaced",
            "Dynamic Animation Replacer (DAR) is no longer updated and doesn't support recent "
            "Skyrim versions. Open Animation Replacer (OAR) loads all DAR animation mods and is "
            "kept up to date.",
            ["Turn DAR off and install Open Animation Replacer (Nexus mod 92109)."]),
    201: ("info", "BodySlide: build your bodies and outfits",
          "Body mods like CBBE and outfit mods made for BodySlide ship without finished meshes; "
          "you build them in BodySlide. Until you do, bodies can look wrong, have gaps at the "
          "neck/wrists, or armour can be invisible.",
          ["Run BodySlide (CalienteTools/BodySlide/BodySlide x64.exe in the game's Data folder) "
           "through Proton, pick a preset, tick “Build Morphs” if asked, and click “Batch Build”.",
           "Install BodySlide's output as its own mod in Kiln so it wins over the body mods."]),
}


def check_known(ctx: Ctx) -> list[Issue]:
    out = []
    for m in ctx.inst.mods:
        k = KNOWN.get(m.nexus_mod_id or 0)
        if m.enabled and k:
            sev, title, explain, steps = k
            acts = [Action(f"disable:{m.id}", "Turn it off", auto=False)] if sev == "warn" and \
                m.nexus_mod_id == 33746 else []
            out.append(Issue(f"known:{m.nexus_mod_id}", sev, title, explain, list(steps), acts,
                             [m.id], "known"))
    return out


def check_collections(ctx: Ctx) -> list[Issue]:
    """Several collections in one setup: say how much they clash."""
    inst = ctx.inst
    colls = {c["slug"]: c.get("name") or c["slug"] for c in inst.collections}
    if len(colls) < 2:
        return []
    by_coll = {m.id: m.collection for m in inst.mods if m.enabled}
    clashes = 0
    for ids in ctx.conflicts.values():
        cs = {by_coll.get(i) for i in ids if by_coll.get(i)}
        if len(cs) > 1:
            clashes += 1
    return [Issue(
        "mixed_collections", "info" if clashes < 20 else "warn",
        f"{len(colls)} collections are installed together",
        f"You've installed {_list(_q(n) for n in colls.values())}. Each collection is tested on "
        "its own; together they can contain different versions of the same mods, "
        f"or mods that overwrite each other ({clashes} file(s) are provided by more than one "
        "collection). Kiln fixes the clear-cut cases (duplicates, outdated plugins), but "
        "collections made for an older game version are the most common cause of trouble.",
        ["If the game is unstable, try one collection at a time: in the Mods page, filter by "
         "collection and turn the others off (or use a separate profile per collection)."],
        [], [], "collections")]


def check_vcruntime(ctx: Ctx) -> list[Issue]:
    """An old Microsoft C++ runtime in the Proton prefix makes newer DLL mods crash."""
    from . import crashlogs, vcruntime
    inst = ctx.inst
    if not vcruntime.too_old(inst):
        return []
    n = vcruntime.dll_mods(inst)
    if not n:
        return []
    v = ".".join(map(str, vcruntime.prefix_version(inst) or ()))
    try:
        fails = crashlogs.xse_load_failures(ctx)
    except Exception:  # noqa: BLE001
        fails = []
    seen = (f" Last time you played, {len(fails)} plugin(s) crashed while starting because of it: "
            + _list(d for d, _h in fails) + ".") if fails else ""
    mods = list(dict.fromkeys(m.id for d, _h in fails
                              for m in [ctx.owner(f"SKSE/Plugins/{d}") or ctx.owner(f"F4SE/Plugins/{d}")] if m))
    return [Issue(
        "vcruntime", "crash" if fails else "broken",
        f"{inst.game.name}'s Proton folder has an old Microsoft C++ runtime ({v})",
        f"Windows mods that are DLLs (you have {n}) use Microsoft's Visual C++ runtime. The copy in "
        f"this game's Proton folder is version {v}, from 2016; mods built with Visual Studio 2022 "
        "17.10 or later (most plugins updated since mid-2024) need 14.40 or newer and crash while "
        "starting with the old one. Script extenders then report “fatal error occurred while "
        "loading plugin” or Windows error 998." + seen,
        ["Click “Update it”: Kiln downloads Microsoft's current Visual C++ Redistributable "
         "(vc_redist.x64.exe, from aka.ms) and installs it into this game's Proton folder.",
         f"Or in a terminal: protontricks {inst.game.steam_appid} vcrun2022"],
        [Action("vcredist", "Update it", auto=False)], mods, "runtime")]


def check_requirements(ctx: Ctx) -> list[Issue]:
    """Requirements listed on the mods' Nexus pages that this profile lacks."""
    import threading
    from . import logreader, requirements
    inst = ctx.inst
    online = threading.current_thread() is not threading.main_thread()   # never block the window
    out = []
    for e in requirements.check(inst, network=online):
        first = e.needed_by[0].name.split(":")[0].strip()
        who = first if len(e.needed_by) == 1 else f"{first} and {len(e.needed_by) - 1} more mod(s)"
        names = _list((m.name for m in e.needed_by), 6)
        note = f" Nexus says: “{e.notes[0]}”." if e.notes else ""
        unused = logreader.unused_requirement(
            inst, [e.nexus_id] + ([e.have.nexus_mod_id] if e.have is not None and e.have.nexus_mod_id else []))
        if unused:
            out.append(Issue(
                f"req_unused:{e.nexus_id}", "info",
                f"{who} list{'s' if len(e.needed_by) == 1 else ''} {_q(e.name)} as a requirement, but nothing uses it",
                f"{_q(e.name)} is listed on the Nexus page of {names}. {unused} Kiln leaves it off, because "
                "it can stop the game from starting when it has nothing to load.",
                ["If a mod you add later ships settings for it, Kiln will suggest turning it on."],
                [Action(f"add_to_profile:{e.have.id}", f"Turn it on anyway", auto=False)] if e.have else [],
                [m.id for m in e.needed_by], "requirements"))
            continue
        if e.have is not None:
            out.append(Issue(
                f"req:{e.nexus_id}", "broken",
                f"{who} need{'s' if len(e.needed_by) == 1 else ''} {_q(e.name)}, which isn't on in this profile",
                f"{_q(e.name)} is listed as a requirement on the Nexus page of {names}.{note} "
                f"You already have it downloaded "
                f"({_label(e.have)}), but it isn't turned on in this profile, so they can fail, "
                "show an error at start-up, or silently do nothing.",
                [f"Click Fix to add {_q(e.have.name)} to this profile.",
                 "Or turn off the mods that need it."],
                [Action(f"add_to_profile:{e.have.id}", f"Add {e.have.name}",
                        describe=f"Add {e.have.name} to this profile (needed by {who})")],
                [m.id for m in e.needed_by] + [e.have.id], "requirements"))
        else:
            out.append(Issue(
                f"req:{e.nexus_id}", "broken",
                f"{who} need{'s' if len(e.needed_by) == 1 else ''} {_q(e.name)}, which isn't installed",
                f"{_q(e.name)} is listed as a requirement on the Nexus page of {names}.{note} "
                "Without it they can fail, show an error "
                "at start-up, or silently do nothing.",
                [f"Click “Install it”: Kiln gets the file for your game version from Nexus.",
                 "Or turn off the mods that need it."],
                [Action(f"install_nexus:{e.nexus_id}", "Install it", auto=False),
                 Action(f"nexus_page:{e.nexus_id}", "Open its page", auto=False)],
                [m.id for m in e.needed_by], "requirements"))
    return out


def check_logs(ctx: Ctx) -> list[Issue]:
    from . import crashlogs
    try:
        return crashlogs.issues(ctx)
    except Exception:  # noqa: BLE001  (a log we can't read shouldn't break the doctor)
        return []


def check_running(ctx: Ctx) -> list[Issue]:
    from . import launchguard
    if launchguard.game_running(ctx.inst):
        return [Issue("running", "info", f"{ctx.inst.game.name} is running",
                      "Kiln won't change game files while it runs. Fixes that need a deploy are "
                      "applied when you close the game.", [], [], [], "setup")]
    return []


CHECKS = [check_running, check_setup, check_empty, check_misplaced, check_duplicates, check_main_file_off,
          check_dll_clash,
          check_script_extender, check_vcruntime, check_plugins, check_requirements, check_known, check_logs,
          check_collections]


def diagnose(inst: "Instance", dismissed: set[str] | frozenset = frozenset()) -> list[Issue]:
    ctx = Ctx(inst)
    out: list[Issue] = []
    for chk in CHECKS:
        try:
            out += chk(ctx)
        except Exception as e:  # noqa: BLE001
            out.append(Issue(f"check_failed:{chk.__name__}", "info",
                             f"Kiln couldn't finish one of its checks ({chk.__name__[6:]})",
                             f"{type(e).__name__}: {e}", source="doctor"))
    seen, uniq = set(), []
    for i in out:
        if i.key not in seen and i.key not in dismissed:
            seen.add(i.key)
            uniq.append(i)
    return sorted(uniq, key=lambda i: i.rank)


def verdict(issues: list[Issue]) -> tuple[str, str]:
    """(level, one plain sentence about the whole setup)."""
    n = {s: sum(i.severity == s for i in issues) for s in SEVERITY}
    if n["crash"]:
        return "crash", (f"Your game will probably crash: {n['crash']} serious problem(s) found."
                         + (f" Plus {n['broken']} mod(s) that won't work." if n["broken"] else ""))
    if n["broken"]:
        return "broken", f"The game should start, but {n['broken']} thing(s) won't work as intended."
    if n["warn"]:
        return "warn", f"Looks OK. {n['warn']} thing(s) could cause trouble."
    return "ok", "No problems found. Your setup looks healthy."


# ====================================================================== fixes done here
def apply(inst: "Instance", fix: str) -> bool:
    """Apply a data fix. Returns True if it changed something that needs a deploy.
    Raises KeyError for fixes the window has to carry out."""
    kind, _, arg = fix.partition(":")
    if kind == "disable":
        for mid in arg.split(","):
            m = inst.get(mid)
            if m:
                m.enabled = False
        inst.save()
        return True
    if kind == "swap":
        good, bad = (inst.get(x) for x in arg.split(":", 1))
        if good is None or bad is None:
            return False
        inst.add_to_profile([good])          # (also turns it on)
        bad.enabled = False
        inst.save()
        return True
    if kind == "add_to_profile":
        m = inst.get(arg)
        if m:
            inst.add_to_profile([m])
        return True
    if kind == "enable_main":
        main_id, _, patch_id = arg.partition("|")
        main, patch = inst.get(main_id), inst.get(patch_id)
        if main:
            main.enabled = True
            if patch and inst.mods.index(main) > inst.mods.index(patch):
                inst.mods.remove(main)
                inst.mods.insert(inst.mods.index(patch), main)   # just before: the patch still wins
            inst.save()
        return True
    if kind == "enable_mod":
        m = inst.get(arg)
        if m:
            m.enabled = True
            inst.save()
        return True
    if kind in ("enable_plugin", "disable_plugin"):
        for p in inst.plugins:
            if p["name"].lower() == arg.lower():
                p["enabled"] = kind == "enable_plugin"
        inst.save()
        _write_plugins(inst)
        return False
    if kind == "sort_plugins":
        from . import autosort
        inst.plugins = autosort.sort_plugins(inst)
        inst.save()
        _write_plugins(inst)
        return False
    if kind == "win":
        winner, loser = arg.split(":", 1)
        w, lo = inst.get(winner), inst.get(loser)
        if not w or not lo:
            return False
        w.rules = [r for r in w.rules if r.get("mod") != lo.id] + [{"type": "after", "mod": lo.id}]
        lo.rules = [r for r in lo.rules if r.get("mod") != w.id]
        inst.mods.remove(w)
        inst.mods.insert(inst.mods.index(lo) + 1, w)
        inst.save()
        return True
    if kind == "relayout":
        m = inst.get(arg)
        if not m:
            return False
        root = inst.mod_path(m)
        inst.forget_files(m.id)
        dirs = [c for c in root.iterdir() if c.is_dir() and not c.name.startswith(".")]
        if len(dirs) != 1:
            return False
        inner = dirs[0]
        tmp = root.with_name(root.name + ".kiln-relayout")
        inner.rename(tmp)                     # avoid clashes when inner has a child of the same name
        for c in tmp.iterdir():
            dest = root / c.name
            if dest.exists():
                continue
            shutil.move(str(c), str(dest))
        shutil.rmtree(tmp, ignore_errors=True)
        return True
    raise KeyError(fix)


def _write_plugins(inst):
    if inst.game.has_plugins:
        try:
            inst.game.write_plugins_txt(inst, [(p["name"], p["enabled"]) for p in inst.plugins])
        except OSError:
            pass


def plan_fix_all(issues: list[Issue]) -> list[tuple[Issue, Action]]:
    """The fixes “Fix everything” performs, in a safe order: mod list changes first,
    then plugins, then deploy."""
    order = {"disable": 0, "enable_mod": 0, "enable_main": 0, "win": 1, "relayout": 1, "enable_plugin": 3,
             "disable_plugin": 3, "sort_plugins": 4, "deploy": 9}
    picks = [(i, i.auto_action) for i in issues if i.auto_action]
    return sorted(picks, key=lambda p: order.get(p[1].fix.split(":")[0], 5))



# ====================================================================== finding a working version
WORDS_NEW = ("latest", "anniversary", " ae", "ae ", "(ae", "ng", "1.6.1170", "1.6.1179")
WORDS_OLD = ("1.5.97", "special edition only", " se only", "pre-ae", "pre ae", " vr", "(vr",
             "skyrim vr", "1.6.640", "1.6.629", "1.6.353", "1.6.318", "legacy", "old version",
             "downgrade")


def find_compatible(inst: "Instance", mod: "Mod"):
    """Look through the mod's Nexus files (older ones included) for the one made for
    the installed game version. Returns (NexusFile | None, plain-English reason)."""
    from . import nexus, versionmatch
    files = nexus.mod_files(inst.game.nexus_game_id, mod.nexus_mod_id)
    rt = runtime(inst)
    if rt:
        f, sure = versionmatch.best_file(files, rt, mod)
        rts = ".".join(map(str, rt))
        if f is None:
            return None, sure
        if f.file_id == mod.nexus_file_id:
            return None, (f"You already have the best match on the mod's page, but it doesn't support "
                          f"{rts}. The author hasn't made a version for it yet.")
        how = {"exact": f"made for {rts}", "likely": f"covers {rts}",
               "guess": f"the newest file; it doesn't say which game versions it supports"}[sure]
        return f, f"“{f.name}” {f.version} ({how})"
    mine = next((f for f in files if f.file_id == mod.nexus_file_id), None)
    better = [f for f in files if f.current and f.category in ("MAIN", "UPDATE")
              and f.file_id != mod.nexus_file_id and (not mine or f.date >= mine.date)]
    if not better:
        return None, "There's no newer file on the mod's page."
    best = max(better, key=lambda f: f.date)
    return best, f"“{best.name}” {best.version}".strip()
