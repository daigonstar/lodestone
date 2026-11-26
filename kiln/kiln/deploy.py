"""Hardlink deployment from staging into the game folder (Vortex-style).

Every file we place is recorded in a manifest so it can be removed again
(purge). Game files we have to overwrite are moved to a backup folder and
restored when no mod provides them any more. Paths are matched
case-insensitively onto existing folders, because the games are Windows games
running under Proton.
"""
from __future__ import annotations

import errno
import json
import os
import shutil
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING, Callable

if TYPE_CHECKING:
    from .instance import Instance

MANIFEST = ".kiln-deployment.json"
BACKUP_DIR = ".kiln-backup"


@dataclass
class DeployResult:
    linked: int = 0
    copied: int = 0
    kept: int = 0
    removed: int = 0
    backed_up: int = 0
    restored: int = 0
    warnings: list[str] = field(default_factory=list)

    def summary(self) -> str:
        parts = [f"{self.linked + self.copied + self.kept} files deployed"]
        if self.copied:
            parts.append(f"{self.copied} copied (hardlink not possible)")
        if self.removed:
            parts.append(f"{self.removed} removed")
        if self.backed_up:
            parts.append(f"{self.backed_up} game files backed up")
        if self.restored:
            parts.append(f"{self.restored} restored")
        return ", ".join(parts)


class CaseMap:
    """Maps a wanted relative path onto existing on-disk casing under root."""

    def __init__(self, root: Path):
        self.root = root
        self.cache: dict[str, dict[str, str]] = {}

    def _listing(self, rel_dir: str) -> dict[str, str]:
        if rel_dir not in self.cache:
            d = self.root / rel_dir if rel_dir else self.root
            try:
                self.cache[rel_dir] = {c.lower(): c for c in os.listdir(d)}
            except OSError:
                self.cache[rel_dir] = {}
        return self.cache[rel_dir]

    def resolve(self, rel: str) -> str:
        out: list[str] = []
        for part in rel.split("/"):
            cur = "/".join(out)
            out.append(self._listing(cur).get(part.lower(), part))
        return "/".join(out)

    def created(self, rel: str) -> None:
        parent, _, name = rel.rpartition("/")
        self._listing(parent)[name.lower()] = name


ROOT_PREFIX = "__root__/"


def deploy_rel(inst: "Instance", staged_rel: str) -> str:
    """Staged path → path relative to the deploy folder. Files a mod keeps under
    __root__/ belong in the game folder itself (e.g. skse64_loader.exe next to
    the game exe while other mods go into Data/)."""
    if staged_rel[:len(ROOT_PREFIX)].lower() != ROOT_PREFIX:
        return staged_rel
    rest = staged_rel[len(ROOT_PREFIX):]
    root = inst.game.deploy_root.strip("/")
    return ("/".join([".."] * (root.count("/") + 1)) + "/" + rest) if root else rest


def _backup_rel(rel: str) -> str:
    """Backups of game-folder files must stay inside the backup folder."""
    parts = rel.split("/")
    return "/".join("__up__" if x == ".." else x for x in parts)


def staged_files(inst: "Instance"):
    """Yield (mod, deploy_rel_posix, abs_path) for enabled mods in priority order."""
    for mod in inst.mods:
        if not mod.enabled:
            continue
        base = inst.mod_path(mod)
        for rel in inst.mod_files(mod):
            yield mod, deploy_rel(inst, rel), base / rel


def plan(inst: "Instance"):
    """Winning source per lowercased path, plus conflicts {key: [mod ids in order]}.
    Remembered until the mod list, its order or a mod's files change."""
    key = (tuple((m.id, m.enabled) for m in inst.mods), inst.files_gen, inst.game.deploy_root)
    memo = getattr(inst, "_plan_memo", None)
    if memo and memo[0] == key:
        return memo[1]
    res = _plan(inst)
    inst._plan_memo = (key, res)
    return res


def _plan(inst: "Instance"):
    desired: dict[str, tuple[str, str, Path]] = {}
    providers: dict[str, list[str]] = {}
    for mod, rel, p in staged_files(inst):
        key = rel.lower()
        desired[key] = (mod.id, rel, p)
        providers.setdefault(key, []).append(mod.id)
    conflicts = {k: v for k, v in providers.items() if len(v) > 1}
    return desired, conflicts


def conflict_summary(inst: "Instance") -> dict[str, dict[str, set[str]]]:
    """{mod_id: {"wins": {other ids it overrides}, "loses": {ids overriding it}}}"""
    _, conflicts = plan(inst)
    out: dict[str, dict[str, set[str]]] = {}
    for ids in conflicts.values():
        winner = ids[-1]
        for loser in ids[:-1]:
            if loser == winner:
                continue
            out.setdefault(winner, {"wins": set(), "loses": set()})["wins"].add(loser)
            out.setdefault(loser, {"wins": set(), "loses": set()})["loses"].add(winner)
    return out


def _load_manifest(inst: "Instance") -> dict:
    try:
        m = json.loads((inst.staging / MANIFEST).read_text())
    except (OSError, ValueError):
        m = {}
    m.setdefault("files", {})
    m.setdefault("backups", {})
    m.setdefault("dirs", [])
    return m


def _save_manifest(inst: "Instance", m: dict) -> None:
    f = inst.staging / MANIFEST
    tmp = f.with_suffix(".tmp")
    tmp.write_text(json.dumps(m))
    tmp.replace(f)


def _is_ours(target: Path, ent: dict) -> bool:
    try:
        st = target.lstat()
    except OSError:
        return False
    if ent.get("mode") == "link":
        return st.st_ino == ent.get("ino")
    return st.st_size == ent.get("size") and int(st.st_mtime) == ent.get("mtime")


def _restore_backup(inst: "Instance", root: Path, rel: str, res: DeployResult) -> None:
    src = inst.staging / BACKUP_DIR / _backup_rel(rel)
    if src.is_file():
        dst = root / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        if dst.exists():
            dst.unlink()
        shutil.move(str(src), str(dst))
        res.restored += 1


def _remove_dirs(root: Path, dirs: list[str], keep: set[str]) -> list[str]:
    """Remove directories we created if empty; returns the ones still present."""
    left = []
    for rel in sorted(set(dirs), key=lambda d: d.count("/"), reverse=True):
        p = root / rel
        if rel in keep:
            left.append(rel)
            continue
        try:
            p.rmdir()
        except OSError:
            if p.is_dir():
                left.append(rel)
    return left


class DeployError(RuntimeError):
    pass


def deploy(inst: "Instance", progress: Callable[[int, int], None] | None = None) -> DeployResult:
    why = inst.game.not_ready(inst)
    if why:
        raise DeployError(why)
    inst.forget_files()          # pick up anything changed in the staging folders by hand
    res = DeployResult()
    root = inst.deploy_dir
    root.mkdir(parents=True, exist_ok=True)
    m = _load_manifest(inst)
    m.pop("vanilla", None)              # a full deploy replaces a vanilla one
    old_files: dict[str, dict] = m["files"]
    backups: dict[str, str] = m["backups"]
    desired, _ = plan(inst)
    # a mod now provides a settings file Kiln had stood in for with an empty one: make way
    gen = m.get("generated", [])
    for name in [n for n in gen if n.lower() in desired]:
        p = root / name
        if p.is_file() and p.stat().st_size == 0:
            p.unlink()
        gen.remove(name)
    cmap = CaseMap(root)

    # 1. remove files we deployed that no mod provides any more
    for key, ent in list(old_files.items()):
        if key in desired:
            continue
        target = root / ent["path"]
        if _is_ours(target, ent):
            target.unlink()
            res.removed += 1
        elif target.exists():
            res.warnings.append(f"Left {ent['path']} in place: it was changed outside Kiln.")
        del old_files[key]
        if key in backups:
            _restore_backup(inst, root, backups.pop(key), res)

    # 2. link the winners
    created_dirs: set[str] = set(m["dirs"])
    total = len(desired)
    new_files: dict[str, dict] = {}
    for i, (key, (mod_id, rel, src)) in enumerate(desired.items(), 1):
        if progress and i % 200 == 0:
            progress(i, total)
        ent = old_files.get(key)
        target_rel = ent["path"] if ent else cmap.resolve(rel)
        target = root / target_rel
        src_st = src.stat()
        if ent and ent.get("mode") == "link" and _is_ours(target, ent) and ent["ino"] == src_st.st_ino:
            new_files[key] = ent
            res.kept += 1
            continue
        if target.exists() or target.is_symlink():
            if ent and _is_ours(target, ent):
                target.unlink()
            elif key not in backups:
                bdst = inst.staging / BACKUP_DIR / _backup_rel(target_rel)
                bdst.parent.mkdir(parents=True, exist_ok=True)
                shutil.move(str(target), str(bdst))
                backups[key] = target_rel
                res.backed_up += 1
            else:
                target.unlink()  # stale file over an existing backup
        # create parent dirs, remembering which ones are ours
        parts = target_rel.split("/")[:-1]
        for j in range(1, len(parts) + 1):
            drel = "/".join(parts[:j])
            d = root / drel
            if not d.is_dir():
                d.mkdir()
                created_dirs.add(drel)
                cmap.created(drel)
        try:
            os.link(src, target)
            mode = "link"
            res.linked += 1
        except OSError as e:
            if e.errno not in (errno.EXDEV, errno.EPERM, errno.EMLINK, errno.ENOTSUP):
                raise
            shutil.copy2(src, target)
            mode = "copy"
            res.copied += 1
        cmap.created(target_rel)
        st = target.lstat()
        new_files[key] = {"path": target_rel, "mod": mod_id, "mode": mode, "ino": st.st_ino,
                          "size": st.st_size, "mtime": int(st.st_mtime)}

    # 3. tidy up directories we created that are now empty
    needed = {"/".join(e["path"].split("/")[:k]) for e in new_files.values()
              for k in range(1, e["path"].count("/") + 1)}
    m["dirs"] = _remove_dirs(root, list(created_dirs), needed)
    m["files"] = new_files
    m["backups"] = backups
    _save_manifest(inst, m)
    if progress:
        progress(total, total)
    res.warnings += inst.game.post_deploy(inst)
    from . import startup
    if startup.applies(inst):              # empty <plugin>.ini files: much faster start-up under Proton
        try:
            startup.empty_plugin_inis(inst, m)
            _save_manifest(inst, m)
        except OSError as e:
            res.warnings.append(f"Couldn't add the start-up speed-up files: {e}")
    try:                                   # each profile's own saves folder (if that's on)
        from . import savegames
        if savegames.supported(inst):
            savegames.point_game(inst)
    except OSError as e:
        res.warnings.append(f"Couldn't point the game at this profile's saves: {e}")
    return res


def purge(inst: "Instance") -> DeployResult:
    """Remove everything Kiln deployed and restore backed-up game files."""
    res = DeployResult()
    root = inst.deploy_dir
    m = _load_manifest(inst)
    for key, ent in m["files"].items():
        target = root / ent["path"]
        if _is_ours(target, ent):
            target.unlink()
            res.removed += 1
        elif target.exists():
            res.warnings.append(f"Left {ent['path']} in place: it was changed outside Kiln.")
    for key, rel in m["backups"].items():
        _restore_backup(inst, root, rel, res)
    from . import startup
    res.removed += startup.remove_generated(inst, m)
    left = _remove_dirs(root, m["dirs"], set())
    _save_manifest(inst, {"files": {}, "backups": {}, "dirs": left})
    return res


def is_deployed(inst: "Instance") -> bool:
    """True when the full mod setup is deployed (a vanilla deploy doesn't count)."""
    m = _load_manifest(inst)
    return bool(m["files"]) and not m.get("vanilla")


def is_vanilla(inst: "Instance") -> bool:
    return bool(_load_manifest(inst).get("vanilla"))


def loader_mod_ids(inst: "Instance") -> set[str]:
    """Enabled mods that provide a mod loader's files (SKSE, SMAPI, CET...). Steam's launch
    options call these loaders, so the game can't start without them."""
    from .launchguard import LOADERS
    wanted = {os.path.normpath(inst.game_dir / f).lower() for ld in LOADERS.get(inst.game.id, []) for f in ld.files}
    keep = set()
    for mod in inst.mods:
        if not mod.enabled:
            continue
        # loader files live in the game folder, so their deploy path looks like "Data/../x.exe"
        if any(os.path.normpath(inst.deploy_dir / deploy_rel(inst, rel)).lower() in wanted for rel in inst.mod_files(mod)):
            keep.add(mod.id)
    return keep


def deploy_vanilla(inst: "Instance") -> DeployResult:
    """Play without mods: take every mod out, keep only the loaders Steam's launch options
    need (they load nothing without mods), and switch every mod plugin off.
    The mod list, its enabled states and the plugin load order are left exactly as they were;
    the manifest is marked so the next modded Play redeploys everything."""
    import copy
    keep = loader_mod_ids(inst)
    enabled = {m.id: m.enabled for m in inst.mods}
    plugins = copy.deepcopy(inst.plugins)
    try:
        purge(inst)
        for m in inst.mods:
            m.enabled = m.id in keep
        res = deploy(inst)
    finally:
        for m in inst.mods:
            m.enabled = enabled[m.id]
        inst.plugins = plugins           # deploy() re-syncs plugins to what's on disk; undo that
        inst._plan_memo = None
    if inst.game.has_plugins:
        inst.game.write_plugins_txt(inst, [(p["name"], False) for p in inst.plugins])
    m = _load_manifest(inst)
    m["vanilla"] = True
    _save_manifest(inst, m)
    return res
