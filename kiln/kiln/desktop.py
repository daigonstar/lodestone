"""Desktop entry + nxm:// URL handler registration (what Vortex/MO2 do on Windows)."""
from __future__ import annotations

import os
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

DESKTOP_ID = "kiln.desktop"
SCHEME = "x-scheme-handler/nxm"


def _apps_dir() -> Path:
    d = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share") / "applications"
    d.mkdir(parents=True, exist_ok=True)
    return d


def exec_line() -> str:
    pkg_parent = Path(__file__).resolve().parent.parent
    return (f"env PYTHONPATH={shlex.quote(str(pkg_parent))} "
            f"{shlex.quote(sys.executable)} -m kiln %u")


def write_desktop_entry(handle_nxm: bool) -> Path:
    install_icon("kiln")
    path = _apps_dir() / DESKTOP_ID
    mime = f"MimeType={SCHEME};\n" if handle_nxm else ""
    path.write_text(
        "[Desktop Entry]\nType=Application\nName=Kiln\n"
        "Comment=Mod manager with Nexus Mods integration\n"
        f"Exec={exec_line()}\nIcon=kiln\nTerminal=false\nStartupWMClass=kiln\n"
        "Categories=Game;Utility;\n" + mime)
    if shutil.which("update-desktop-database"):
        subprocess.run(["update-desktop-database", str(_apps_dir())], check=False, capture_output=True)
    return path


def current_nxm_handler() -> str:
    if not shutil.which("xdg-mime"):
        return ""
    p = subprocess.run(["xdg-mime", "query", "default", SCHEME], capture_output=True, text=True)
    return p.stdout.strip()


def register_nxm() -> Path:
    path = write_desktop_entry(handle_nxm=True)
    if shutil.which("xdg-mime"):
        subprocess.run(["xdg-mime", "default", DESKTOP_ID, SCHEME], check=False)
    return path


def unregister_nxm(previous: str = "") -> None:
    write_desktop_entry(handle_nxm=False)
    if previous and previous != DESKTOP_ID and shutil.which("xdg-mime"):
        subprocess.run(["xdg-mime", "default", previous, SCHEME], check=False)


def is_nxm_handler() -> bool:
    return current_nxm_handler() == DESKTOP_ID


def install_icon(name: str) -> None:
    """Copy the app logo into the user's hicolor icon theme (scalable + PNG sizes)."""
    import os
    src = Path(__file__).resolve().parent / "assets" / f"{name}.svg"
    base = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share") / "icons/hicolor"
    d = base / "scalable/apps"
    d.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(src, d / f"{name}.svg")
    if shutil.which("rsvg-convert"):
        for sz in (16, 24, 32, 48, 64, 128, 256, 512):
            out = base / f"{sz}x{sz}/apps"
            out.mkdir(parents=True, exist_ok=True)
            subprocess.run(["rsvg-convert", "-w", str(sz), "-h", str(sz), str(src), "-o",
                            str(out / f"{name}.png")], check=False, capture_output=True)
    if shutil.which("gtk-update-icon-cache"):
        subprocess.run(["gtk-update-icon-cache", "-f", "-t", str(base)], check=False, capture_output=True)
