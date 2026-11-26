"""Read and write a game's Steam launch options (userdata/<id>/config/localconfig.vdf).

Steam keeps this file in memory and rewrites it on exit, so changes are only
written while Steam isn't running. A backup is made before every write.
"""
from __future__ import annotations

import re
import shutil
import subprocess
import time
from pathlib import Path

from . import steam


# ------------------------------------------------------------------ tiny VDF (text KeyValues)
def _tokens(text: str):
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c in " \t\r\n":
            i += 1
        elif c == "/" and text.startswith("//", i):
            i = text.find("\n", i)
            i = n if i < 0 else i
        elif c in "{}":
            yield c
            i += 1
        elif c == '"':
            j, buf = i + 1, []
            while j < n and text[j] != '"':
                if text[j] == "\\" and j + 1 < n:
                    buf.append({"n": "\n", "t": "\t"}.get(text[j + 1], text[j + 1]))
                    j += 2
                else:
                    buf.append(text[j])
                    j += 1
            yield ("s", "".join(buf))
            i = j + 1
        else:
            j = i
            while j < n and text[j] not in " \t\r\n{}\"":
                j += 1
            yield ("s", text[i:j])
            i = j


def loads(text: str) -> dict:
    stack = [{}]
    key = None
    for t in _tokens(text):
        if t == "{":
            d: dict = {}
            stack[-1][key] = d
            stack.append(d)
            key = None
        elif t == "}":
            stack.pop()
        elif key is None:
            key = t[1]
        else:
            stack[-1][key] = t[1]
            key = None
    return stack[0]


def _q(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def dumps(d: dict, depth: int = 0) -> str:
    out, ind = [], "\t" * depth
    for k, v in d.items():
        if isinstance(v, dict):
            out.append(f"{ind}{_q(k)}\n{ind}{{\n{dumps(v, depth + 1)}{ind}}}\n")
        else:
            out.append(f"{ind}{_q(k)}\t\t{_q(v)}\n")
    return "".join(out)


# ------------------------------------------------------------------ localconfig.vdf
def localconfig() -> Path | None:
    """The localconfig.vdf of the Steam account used most recently."""
    files = [f for r in steam.steam_roots() for f in (r / "userdata").glob("*/config/localconfig.vdf")]
    return max(files, key=lambda f: f.stat().st_mtime) if files else None


def _apps(d: dict) -> dict:
    node = d
    for k in ("UserLocalConfigStore", "Software", "Valve", "Steam", "apps"):
        node = next((v for kk, v in node.items() if kk.lower() == k.lower() and isinstance(v, dict)), None)
        if node is None:
            return {}
    return node


def get_launch_options(appid: int) -> str:
    f = localconfig()
    if not f:
        return ""
    app = _apps(loads(f.read_text(encoding="utf-8", errors="replace"))).get(str(appid)) or {}
    return app.get("LaunchOptions", "") if isinstance(app, dict) else ""


def steam_running() -> bool:
    try:
        return subprocess.run(["pgrep", "-x", "steam"], capture_output=True).returncode == 0
    except FileNotFoundError:
        return False


def shutdown_steam(timeout: float = 40) -> bool:
    """Ask Steam to exit and wait for it. Returns True once it's gone."""
    if not steam_running():
        return True
    subprocess.run(["steam", "-shutdown"], capture_output=True, timeout=20)
    end = time.time() + timeout
    while time.time() < end:
        if not steam_running():
            time.sleep(1.5)       # let it finish writing its own files
            return True
        time.sleep(0.5)
    return False


def set_launch_options(appid: int, value: str) -> Path:
    """Write the launch options for one game. Steam must not be running."""
    if steam_running():
        raise RuntimeError("Steam is running. It would overwrite the change when it exits.")
    f = localconfig()
    if not f:
        raise RuntimeError("Couldn't find Steam's localconfig.vdf.")
    text = f.read_text(encoding="utf-8", errors="replace")
    d = loads(text)
    apps = _apps(d)
    if not apps and "UserLocalConfigStore" not in d:
        raise RuntimeError("localconfig.vdf has an unexpected layout; not touching it.")
    if not apps:
        node = d
        for k in ("UserLocalConfigStore", "Software", "Valve", "Steam", "apps"):
            node = node.setdefault(k, {})
        apps = node
    app = apps.setdefault(str(appid), {})
    app["LaunchOptions"] = value
    backup = f.with_name(f"localconfig.vdf.kiln-{time.strftime('%Y%m%d-%H%M%S')}.bak")
    shutil.copy2(f, backup)
    tmp = f.with_suffix(".kiln-tmp")
    tmp.write_text(dumps(d), encoding="utf-8")
    loads(tmp.read_text(encoding="utf-8"))      # sanity: it parses back
    tmp.replace(f)
    return backup


# ------------------------------------------------------------------ composing options
def has_env(options: str, var: str) -> str | None:
    m = re.search(rf'(?:^|\s){var}=("([^"]*)"|\S*)', options)
    return (m.group(2) if m.group(2) is not None else m.group(1)) if m else None


def with_dll_overrides(options: str, dlls: list[str]) -> str:
    """Add n,b overrides for dlls, keeping whatever else the options contain."""
    cur = has_env(options, "WINEDLLOVERRIDES")
    have = set()
    entries = []
    if cur:
        for part in cur.split(";"):
            names, _, mode = part.partition("=")
            for nm in names.split(","):
                if nm:
                    have.add(nm.strip().lower())
            entries.append(part)
    missing = [d for d in dlls if d.lower() not in have]
    if not missing:
        return options
    entries.append(",".join(missing) + "=n,b")
    env = 'WINEDLLOVERRIDES="' + ";".join(e for e in entries if e) + '"'
    if cur is not None:
        return re.sub(r'(?:^|(?<=\s))WINEDLLOVERRIDES=("[^"]*"|\S*)', lambda _m: env, options, count=1)
    if "%command%" in options:
        return f"{env} {options}".strip()
    return f"{env} %command% {options}".strip()


def with_wrapper(options: str, wrapper: str) -> str:
    """Replace %command% by `wrapper %command%` unless the wrapper is already there."""
    if wrapper in options:
        return options
    if "%command%" in options:
        return options.replace("%command%", f"{wrapper} %command%", 1)
    return f"{wrapper} %command% {options}".strip()
