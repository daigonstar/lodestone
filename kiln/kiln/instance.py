"""A managed game: its folder, staging folder, installed mods and plugin order."""
from __future__ import annotations

import json
import os
import re
import shutil
import tempfile
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path

from . import archive, steam
from .archive import ci_child, ci_resolve
from .games import GameDef, safe_name

STATE_FILE = "kiln.json"
DEFAULT_PROFILE = "Default"


class InstallError(RuntimeError):
    pass


@dataclass
class Mod:
    id: str                      # folder name in staging
    name: str
    version: str = ""
    enabled: bool = True
    nexus_mod_id: int | None = None
    nexus_file_id: int | None = None
    file_name: str = ""
    author: str = ""
    archive: str = ""            # file name in the downloads folder
    installed_at: int = 0
    fomod_choices: dict | None = None
    source_url: str = ""
    collection: str = ""         # slug of the Nexus collection that installed it
    category: str = ""
    notes: str = ""
    summary: str = ""
    picture: str = ""            # Nexus picture URL
    rules: list = field(default_factory=list)   # [{"type": "after"|"before", "mod": id}] kept by auto-sort

    @classmethod
    def from_dict(cls, d: dict) -> "Mod":
        known = {f for f in cls.__dataclass_fields__}
        return cls(**{k: v for k, v in d.items() if k in known})


@dataclass
class Prepared:
    """An extracted archive waiting to be staged (possibly after a FOMOD wizard)."""
    tmp: Path
    root: Path
    name: str
    meta: dict = field(default_factory=dict)
    fomod_xml: Path | None = None
    fomod_base: Path | None = None   # folder FOMOD source paths are relative to

    def cleanup(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)


class Instance:
    def __init__(self, game: GameDef, game_dir: Path, staging: Path):
        self.game = game
        self.game_dir = Path(game_dir)
        self.staging = Path(staging)
        self.staging.mkdir(parents=True, exist_ok=True)
        self.mods: list[Mod] = []
        self.plugins: list[dict] = []    # [{"name":..., "enabled": bool}] in load order
        self.collections: list[dict] = []  # [{"slug", "name", "revision", "installed_at"}]
        # Profiles share the installed mods; each keeps its own enabled set,
        # mod order and plugin load order. The live state is the active one.
        self.profiles: dict[str, dict] = {}
        self.active_profile = DEFAULT_PROFILE
        self.saves: dict = {}
        self._files: dict[str, list[str]] = {}    # mod id -> staged files (relative, posix)
        self.files_gen = 0                         # bumps whenever a cached list is dropped
        self.load()

    # ------------------------------------------------------------- paths
    @staticmethod
    def default_staging(game: GameDef, game_dir: Path) -> Path:
        """Same filesystem as the game so hardlinks work: <library>/kiln/<game>."""
        lib = steam.library_of(game_dir)
        base = lib if lib else Path(game_dir).parent
        return base / "kiln" / game.id

    @property
    def deploy_dir(self) -> Path:
        return self.game.deploy_dir(self)

    @property
    def downloads_dir(self) -> Path:
        d = self.staging / "downloads"
        d.mkdir(exist_ok=True)
        return d

    @property
    def mods_dir(self) -> Path:
        d = self.staging / "mods"
        if not getattr(self, "_mods_dir_made", False):    # checked once, not per mod
            d.mkdir(parents=True, exist_ok=True)
            self._mods_dir_made = True
        return d

    def mod_path(self, mod: Mod) -> Path:
        return self.mods_dir / mod.id

    def mod_files(self, mod: Mod) -> list[str]:
        """The files a mod has staged (relative posix paths). Remembered, because
        walking tens of thousands of files is slow; Kiln drops the list whenever it
        changes a mod's files, and before every deploy."""
        out = self._files.get(mod.id)
        if out is None:
            root = self.mod_path(mod)
            out = []
            for dirpath, _dirs, files in os.walk(root):
                rel = os.path.relpath(dirpath, root).replace("\\", "/")
                out += [f if rel == "." else f"{rel}/{f}" for f in files]
            self._files[mod.id] = out
        return out

    def forget_files(self, mod_id: str | None = None) -> None:
        if mod_id is None:
            self._files.clear()
        else:
            self._files.pop(mod_id, None)
        self.files_gen += 1

    # ------------------------------------------------------------- state
    def load(self) -> None:
        try:
            d = json.loads((self.staging / STATE_FILE).read_text())
        except (OSError, ValueError):
            d = {}
        self.mods = [Mod.from_dict(m) for m in d.get("mods", [])]
        self.mods = [m for m in self.mods if self.mod_path(m).is_dir()]
        self.plugins = d.get("plugins", [])
        self.collections = d.get("collections", [])
        self.profiles = d.get("profiles") or {}
        self.saves = d.get("saves") or {}          # {"per_profile": bool}
        self.active_profile = d.get("active_profile") or DEFAULT_PROFILE
        if self.active_profile not in self.profiles:
            self.profiles[self.active_profile] = self._snapshot()
        self._migrate_members()

    def _migrate_members(self) -> None:
        """Older state files don't say which mods belong to which profile. Each
        profile gets the mods enabled in it, and any mod in no profile goes to the
        profile that was newest when the mod was installed (the one in use then)."""
        if all("members" in p for p in self.profiles.values()):
            return
        for p in self.profiles.values():
            p.setdefault("members", list(p.get("enabled", [])))
        owned = {mid for p in self.profiles.values() for mid in p["members"]}
        by_age = sorted(self.profiles.items(), key=lambda kv: kv[1].get("created", 0))
        for m in self.mods:
            if m.id in owned:
                continue
            home = [n for n, p in by_age if p.get("created", 0) <= (m.installed_at or 0)]
            name = home[-1] if home else (self.active_profile if self.active_profile in self.profiles
                                          else by_age[0][0])
            self.profiles[name]["members"].append(m.id)

    def _snapshot(self) -> dict:
        cur = self.profiles.get(self.active_profile, {})
        have = {m.id for m in self.mods}
        members = [i for i in cur.get("members", []) if i in have]
        members += [m.id for m in self.mods if m.enabled and m.id not in members]
        return {"order": [m.id for m in self.mods], "enabled": [m.id for m in self.mods if m.enabled],
                "plugins": [dict(p) for p in self.plugins], "members": members,
                "created": cur.get("created") or int(time.time())}

    # ------------------------------------------------------------- profile membership
    def members(self, profile: str | None = None) -> set[str]:
        """Ids of the mods that belong to a profile (installed into it or added to it).
        Mods are stored once and can belong to several profiles."""
        name = profile or self.active_profile
        if name == self.active_profile:
            return set(self.profiles.get(name, {}).get("members", [])) | {m.id for m in self.mods if m.enabled}
        return set(self.profiles.get(name, {}).get("members", []))

    def in_profile(self, mod: Mod) -> bool:
        return mod.enabled or mod.id in self.profiles.get(self.active_profile, {}).get("members", [])

    def add_to_profile(self, mods: list[Mod], enable: bool = True) -> None:
        pr = self.profiles.setdefault(self.active_profile, {})
        mem = pr.setdefault("members", [])
        for m in mods:
            if m.id not in mem:
                mem.append(m.id)
            if enable:
                m.enabled = True
        self.save()

    def remove_from_profile(self, mods: list[Mod]) -> None:
        """Take mods out of the active profile (they stay installed for the others)."""
        pr = self.profiles.setdefault(self.active_profile, {})
        ids = {m.id for m in mods}
        pr["members"] = [i for i in pr.get("members", []) if i not in ids]
        for m in mods:
            m.enabled = False
        self.save()

    def profiles_using(self, mod: Mod) -> list[str]:
        return [n for n in self.profile_names() if mod.id in self.members(n)]

    def save(self) -> None:
        self.profiles[self.active_profile] = self._snapshot()
        f = self.staging / STATE_FILE
        tmp = f.with_suffix(".tmp")
        tmp.write_text(json.dumps({"game": self.game.id, "mods": [asdict(m) for m in self.mods],
                                   "plugins": self.plugins, "collections": self.collections,
                                   "profiles": self.profiles, "active_profile": self.active_profile,
                                   "saves": self.saves},
                                  indent=1))
        tmp.replace(f)

    # ------------------------------------------------------------- profiles
    def profile_names(self) -> list[str]:
        return sorted(self.profiles, key=lambda n: (n != DEFAULT_PROFILE, n.lower()))

    def profile_stats(self, name: str) -> tuple[int, int]:
        """(enabled mods, enabled plugins) in a profile."""
        pr = self.profiles.get(name, {})
        have = {m.id for m in self.mods}
        return (sum(1 for i in pr.get("enabled", []) if i in have),
                sum(1 for p in pr.get("plugins", []) if p.get("enabled")))

    def switch_profile(self, name: str) -> None:
        """Make another profile live. Mods installed since it was last used come
        in disabled at the bottom. The caller redeploys."""
        if name not in self.profiles:
            raise InstallError(f"No profile called “{name}”.")
        self.save()
        pr = self.profiles[name]
        rank = {mid: i for i, mid in enumerate(pr.get("order", []))}
        on = set(pr.get("enabled", []))
        self.mods.sort(key=lambda m: (m.id not in rank, rank.get(m.id, 0)))
        for m in self.mods:
            m.enabled = m.id in on
        self.plugins = [dict(p) for p in pr.get("plugins", [])]
        self.active_profile = name
        self.save()

    def create_profile(self, name: str, copy_from: str | None = None) -> None:
        name = name.strip()
        if not name or name in self.profiles:
            raise InstallError(f"A profile called “{name}” already exists." if name else "Give it a name.")
        self.save()
        if copy_from:
            src = self.profiles[copy_from]
            self.profiles[name] = {**json.loads(json.dumps(src)), "created": int(time.time())}
        else:   # empty profile: same mods available, all disabled, no plugins
            self.profiles[name] = {"order": [m.id for m in self.mods], "enabled": [], "plugins": [],
                                   "members": [], "created": int(time.time())}
        self.save()

    def rename_profile(self, old: str, new: str) -> None:
        new = new.strip()
        if not new or new in self.profiles:
            raise InstallError(f"A profile called “{new}” already exists." if new else "Give it a name.")
        self.profiles[new] = self.profiles.pop(old)
        if self.active_profile == old:
            self.active_profile = new
        self.save()

    def delete_profile(self, name: str) -> None:
        if name == self.active_profile:
            raise InstallError("Switch to another profile before deleting this one.")
        if len(self.profiles) <= 1:
            raise InstallError("Kiln needs at least one profile.")
        self.profiles.pop(name, None)
        self.save()

    def get(self, mod_id: str) -> Mod | None:
        return next((m for m in self.mods if m.id == mod_id), None)

    def find_nexus(self, nexus_mod_id: int) -> Mod | None:
        """Any installed file of a Nexus mod (a mod page can have several files)."""
        return next((m for m in self.mods if m.nexus_mod_id == nexus_mod_id), None)

    def find_nexus_file(self, nexus_mod_id: int, file_id: int | None = None,
                        file_name: str = "") -> Mod | None:
        """The installed mod a download of this Nexus file would update: the same
        file, or a file with the same name minus version numbers. Files of the same
        mod page with other names (main vs optional files) are separate mods."""
        same = [m for m in self.mods if m.nexus_mod_id == nexus_mod_id]
        if not same:
            return None
        if file_id:
            hit = next((m for m in same if m.nexus_file_id == file_id), None)
            if hit:
                return hit
        norm = lambda s: " ".join(re.sub(r"\bv?\d+(?:[._-]\d+)*[a-z]?\b", " ", (s or "").lower()).split())
        if file_name:
            hit = next((m for m in same if m.file_name and norm(m.file_name) == norm(file_name)), None)
            if hit:
                return hit
            # older installs didn't record the file name
            legacy = [m for m in same if not m.file_name]
            return legacy[0] if len(same) == 1 and legacy else None
        return same[0] if len(same) == 1 else None

    # ------------------------------------------------------------- install
    def prepare(self, archive_path: Path, name: str, meta: dict | None = None) -> Prepared:
        """Extract an archive to a temp dir on the staging filesystem and detect FOMOD."""
        tmp = Path(tempfile.mkdtemp(prefix=".install-", dir=self.staging))
        root = tmp / "x"
        try:
            if Path(archive_path).is_dir():   # bundled folder inside a collection
                shutil.copytree(archive_path, root)
            else:
                archive.extract(archive_path, root)
        except Exception:
            shutil.rmtree(tmp, ignore_errors=True)
            raise
        p = Prepared(tmp=tmp, root=root, name=name, meta=dict(meta or {}))
        # FOMOD: fomod/ModuleConfig.xml at the root or inside a wrapper folder
        from .games import walk_dirs
        for d in walk_dirs(root, 2):
            fdir = ci_child(d, "fomod")
            if fdir is not None and fdir.is_dir():
                cfg = ci_child(fdir, "ModuleConfig.xml")
                if cfg is not None:
                    p.fomod_xml, p.fomod_base = cfg, d
                    break
        return p

    def _new_mod_id(self, name: str, replacing: Mod | None) -> str:
        if replacing:
            return replacing.id
        base = safe_name(name)
        mid, n = base, 2
        while self.get(mid) or (self.mods_dir / mid).exists():
            mid = f"{base} ({n})"
            n += 1
        return mid

    def finish_install(self, prep: Prepared, fomod_files: list[tuple[str, str, bool]] | None = None,
                       fomod_choices: dict | None = None) -> Mod:
        """Stage a prepared archive. fomod_files: [(source, destination, is_folder)]
        relative to the FOMOD base / deploy root."""
        try:
            meta = prep.meta
            replacing = (self.find_nexus_file(meta["nexus_mod_id"], None, meta.get("file_name", ""))
                         if meta.get("nexus_mod_id") else None)
            if meta.get("replace_id"):
                replacing = self.get(meta["replace_id"]) or replacing
            build = prep.tmp / "staged"
            build.mkdir()
            if fomod_files is not None:
                base = self.game.fomod_base()
                for src, dst, is_folder in fomod_files:
                    sp = ci_resolve(prep.fomod_base, src) if src else prep.fomod_base
                    if sp is None:
                        raise InstallError(f"FOMOD refers to a missing file: {src}")
                    dst = dst.replace("\\", "/").strip("/")
                    if not is_folder and (not dst or dst.endswith("/")):
                        dst = f"{dst}/{sp.name}".strip("/")
                    target = build / base / dst if base else build / dst
                    archive.copy_into(sp, target)
            else:
                layout = self.game.layout(prep.root, prep.name)
                if not layout:
                    raise InstallError(
                        f"Kiln couldn't work out where this mod goes in {self.game.name}. "
                        "Open the archive and check its readme; it may need a tool or a manual "
                        "install.")
                for src, dst in layout:
                    archive.merge_into(src, build / dst if dst else build)
            if not any(build.rglob("*")):
                raise InstallError("The mod installed no files.")

            mod_id = self._new_mod_id(meta.get("name") or prep.name, replacing)
            dest = self.mods_dir / mod_id
            if dest.exists():
                shutil.rmtree(dest)
            build.rename(dest)
            self.forget_files(mod_id)
            mod = Mod(
                id=mod_id, name=meta.get("name") or prep.name, version=meta.get("version", ""),
                # a reinstall / version swap / collection re-run keeps the old copy's state;
                # installing a mod yourself means you want it on, even if Kiln had it switched off
                enabled=replacing.enabled if (replacing and meta.get("replace_id")) else True,
                nexus_mod_id=meta.get("nexus_mod_id"), nexus_file_id=meta.get("nexus_file_id"),
                file_name=meta.get("file_name", ""), author=meta.get("author", ""),
                archive=meta.get("archive", ""), installed_at=int(time.time()),
                fomod_choices=fomod_choices, source_url=meta.get("source_url", ""),
                collection=meta.get("collection") or (replacing.collection if replacing else ""),
                category=meta.get("category") or (replacing.category if replacing else ""),
                notes=replacing.notes if replacing else "",
                summary=meta.get("summary") or (replacing.summary if replacing else ""),
                picture=meta.get("picture") or (replacing.picture if replacing else ""),
            )
            if replacing and replacing.name != (meta.get("name") or prep.name) and meta.get("replace_id"):
                mod.name = replacing.name        # keep a name the user chose
            if replacing:
                self.mods[self.mods.index(replacing)] = mod
                for pr in self.profiles.values():
                    for key in ("members", "enabled", "order"):
                        pr[key] = [mod_id if i == replacing.id else i for i in pr.get(key, [])]
            else:
                self.mods.append(mod)
            mem = self.profiles.setdefault(self.active_profile, {}).setdefault("members", [])
            if mod.id not in mem:
                mem.append(mod.id)
            self.save()
            return mod
        finally:
            prep.cleanup()

    def remove(self, mod: Mod) -> None:
        p = self.mod_path(mod).resolve()
        if self.mods_dir.resolve() not in p.parents:
            raise InstallError("Refusing to delete outside the staging folder.")
        shutil.rmtree(p, ignore_errors=True)
        self.forget_files(mod.id)
        self.mods.remove(mod)
        for pr in self.profiles.values():
            for key in ("members", "enabled", "order"):
                pr[key] = [i for i in pr.get(key, []) if i != mod.id]
        self.save()

    def move(self, mod: Mod, new_index: int) -> None:
        self.mods.remove(mod)
        self.mods.insert(max(0, min(new_index, len(self.mods))), mod)
        self.save()

    # ------------------------------------------------------------- plugins
    def present_plugins(self) -> list[str]:
        from .games.bethesda import PLUGIN_EXTS
        d = self.deploy_dir
        if not d.is_dir():
            return []
        return sorted((c.name for c in d.iterdir() if c.is_file() and c.suffix.lower() in PLUGIN_EXTS),
                      key=str.lower)

    def sync_plugins(self) -> None:
        """Reconcile the plugin list with plugin files in Data. New plugins are
        enabled; masters are inserted after the last master."""
        if not self.game.has_plugins:
            return
        from .games.bethesda import read_plugin
        implicit = {n.lower() for n in self.game.implicit_plugins(self.game_dir)}
        present = {n.lower(): n for n in self.present_plugins() if n.lower() not in implicit}
        if not self.plugins:  # first run: adopt the order already in plugins.txt
            self.plugins = [{"name": n, "enabled": e} for n, e in self.game.read_plugins_txt(self)
                            if n.lower() in present]
        known = {p["name"].lower() for p in self.plugins}
        self.plugins = [p for p in self.plugins if p["name"].lower() in present]
        # New plugins in dependency order: each after the masters it lists.
        new = {low: name for low, name in present.items() if low not in known}
        masters = {low: [m.lower() for m in read_plugin(self.deploy_dir / name).masters]
                   for low, name in new.items()}
        ordered: list[str] = []

        def visit(low, seen=()):
            if low in ordered or low in seen or low not in new:
                return
            for m in masters[low]:
                visit(m, seen + (low,))
            ordered.append(low)

        for low in sorted(new):
            visit(low)
        for low in ordered:
            name = new[low]
            entry = {"name": name, "enabled": True}
            if read_plugin(self.deploy_dir / name).is_master:
                idx = max((i + 1 for i, p in enumerate(self.plugins)
                           if read_plugin(self.deploy_dir / p["name"]).is_master), default=0)
                self.plugins.insert(idx, entry)
            else:
                self.plugins.append(entry)
        self.save()

    def apply_plugin_order(self, wanted: list[tuple[str, bool]]) -> None:
        """Reorder/enable plugins to match a collection's load order; plugins the
        collection doesn't mention keep their relative position at the end."""
        rank = {n.lower(): i for i, (n, _) in enumerate(wanted)}
        enabled = {n.lower(): e for n, e in wanted}
        for p in self.plugins:
            if p["name"].lower() in enabled:
                p["enabled"] = enabled[p["name"].lower()]
        self.plugins.sort(key=lambda p: rank.get(p["name"].lower(), len(rank)))
        self.save()

    def sort_plugins_masters_first(self) -> None:
        from .games.bethesda import read_plugin
        info = {p["name"]: read_plugin(self.deploy_dir / p["name"]) for p in self.plugins}
        self.plugins.sort(key=lambda p: 0 if info[p["name"]].is_master else 1)
        self.save()

    def missing_master_warnings(self) -> list[str]:
        from .games.bethesda import read_plugin
        avail = {n.lower() for n in self.game.implicit_plugins(self.game_dir)}
        avail |= {p["name"].lower() for p in self.plugins if p["enabled"]}
        out = []
        for p in self.plugins:
            if not p["enabled"]:
                continue
            missing = [m for m in read_plugin(self.deploy_dir / p["name"]).masters
                       if m.lower() not in avail]
            if missing:
                out.append(f"{p['name']} needs missing or disabled master(s): {', '.join(missing)}")
        return out

    # ------------------------------------------------------------- downloads
    def downloads(self) -> list[tuple[Path, dict]]:
        out = []
        for f in sorted(self.downloads_dir.iterdir(), key=lambda p: p.stat().st_mtime, reverse=True):
            if f.is_file() and not f.name.endswith((".json", ".part")):
                meta = {}
                side = f.with_name(f.name + ".json")
                if side.is_file():
                    try:
                        meta = json.loads(side.read_text())
                    except ValueError:
                        pass
                out.append((f, meta))
        return out

    @staticmethod
    def write_download_meta(archive_path: Path, meta: dict) -> None:
        archive_path.with_name(archive_path.name + ".json").write_text(json.dumps(meta, indent=1))
