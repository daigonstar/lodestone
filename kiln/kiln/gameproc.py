"""The running game: its processes, whether it still has a window, and ending it.

Proton games sometimes keep running after you quit (Skyrim is famous for it):
the window is gone but the process hangs, so Steam still shows “Running” and
won't start the game again. Kiln watches for that and ends the leftover process.
"""
from __future__ import annotations

import json
import os
import shutil
import signal
import subprocess
import time
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .instance import Instance


def game_pids(inst: "Instance") -> list[int]:
    """Processes that ARE the game (its exe), not Steam's launch helpers around it."""
    exe = Path(inst.game.exe).name.lower() if inst.game.exe else ""
    if not exe:
        return []
    names = {exe, exe.replace(".exe", "")}
    out = []
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            argv0 = Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0", 1)[0].decode(errors="replace")
            comm = Path(f"/proc/{pid}/comm").read_text().strip().lower()
        except OSError:
            continue
        base = argv0.replace("\\", "/").rsplit("/", 1)[-1].lower()
        if base in names or comm in names or (len(comm) == 15 and exe.startswith(comm)):
            out.append(int(pid))
    return out


def has_window(pids: list[int]) -> bool | None:
    """Does any of these processes have a window? None when Kiln can't tell."""
    if not pids:
        return False
    if os.environ.get("HYPRLAND_INSTANCE_SIGNATURE") and shutil.which("hyprctl"):
        try:
            out = subprocess.run(["hyprctl", "clients", "-j"], capture_output=True, timeout=3).stdout
            return any(c.get("pid") in pids and c.get("mapped", True) for c in json.loads(out or b"[]"))
        except (OSError, ValueError, subprocess.SubprocessError):
            return None
    if os.environ.get("DISPLAY") and shutil.which("xdotool"):
        for p in pids:
            try:
                r = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(p)],
                                   capture_output=True, timeout=3)
            except (OSError, subprocess.SubprocessError):
                return None
            if r.stdout.strip():
                return True
        return False
    return None


def stop(pids: list[int], grace: float = 6.0) -> int:
    """Ask the game to close (SIGTERM), then force it (SIGKILL). Returns how many ended."""
    for p in pids:
        try:
            os.kill(p, signal.SIGTERM)
        except OSError:
            pass
    end = time.time() + grace
    alive = list(pids)
    while alive and time.time() < end:
        time.sleep(0.3)
        alive = [p for p in alive if Path(f"/proc/{p}").exists()]
    for p in alive:
        try:
            os.kill(p, signal.SIGKILL)
        except OSError:
            pass
    return len(pids)
