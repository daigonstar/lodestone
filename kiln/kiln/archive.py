"""Archive extraction (bsdtar handles zip/7z/rar/tar) and case-insensitive path helpers."""
from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path


class ArchiveError(RuntimeError):
    pass


def extract(archive: Path, into: Path) -> None:
    if not shutil.which("bsdtar"):
        raise ArchiveError("bsdtar (libarchive) is required to extract mods.")
    into.mkdir(parents=True, exist_ok=True)
    # bsdtar refuses absolute paths and '..' entries unless -P is given.
    p = subprocess.run(["bsdtar", "-xf", str(archive), "-C", str(into)],
                       capture_output=True, text=True)
    if p.returncode != 0:
        raise ArchiveError(f"Could not extract {archive.name}:\n{p.stderr.strip()}")
    # Make everything readable/writable by us (some archives carry odd modes).
    for root, dirs, files in os.walk(into):
        for n in dirs:
            os.chmod(os.path.join(root, n), 0o755)
        for n in files:
            fp = os.path.join(root, n)
            if not os.path.islink(fp):
                os.chmod(fp, 0o644)


def unwrap(root: Path) -> Path:
    """Descend through single-directory wrappers (Archive/ModName/ModName/...)."""
    cur = root
    while True:
        kids = [k for k in cur.iterdir() if not k.name.startswith("__MACOSX")]
        if len(kids) == 1 and kids[0].is_dir():
            cur = kids[0]
        else:
            return cur


def ci_child(parent: Path, name: str) -> Path | None:
    """Case-insensitive lookup of a direct child."""
    want = name.lower()
    try:
        for c in parent.iterdir():
            if c.name.lower() == want:
                return c
    except OSError:
        pass
    return None


def ci_resolve(base: Path, rel: str) -> Path | None:
    """Resolve a Windows-style relative path case-insensitively under base."""
    cur = base
    for part in rel.replace("\\", "/").split("/"):
        if part in ("", "."):
            continue
        if part == "..":
            return None
        nxt = ci_child(cur, part)
        if nxt is None:
            return None
        cur = nxt
    return cur


def merge_into(src: Path, dest: Path) -> None:
    """Move/merge src (file or dir) into dest, merging directories case-insensitively."""
    if src.is_dir():
        dest.mkdir(parents=True, exist_ok=True)
        for c in src.iterdir():
            existing = ci_child(dest, c.name)
            merge_into(c, existing if existing is not None else dest / c.name)
    else:
        dest.parent.mkdir(parents=True, exist_ok=True)
        if dest.exists():
            dest.unlink()
        shutil.move(str(src), str(dest))


def copy_into(src: Path, dest: Path) -> None:
    """Like merge_into but copies (source tree is kept)."""
    if src.is_dir():
        dest.mkdir(parents=True, exist_ok=True)
        for c in src.iterdir():
            existing = ci_child(dest, c.name)
            copy_into(c, existing if existing is not None else dest / c.name)
    else:
        dest.parent.mkdir(parents=True, exist_ok=True)
        if dest.exists():
            dest.unlink()
        shutil.copy2(src, dest)
