#!/usr/bin/env python3
"""Fail if Lodestone's code mentions a web address that isn't in network-hosts.txt.

Lodestone promises (README, "Security and privacy") that it only talks to the services listed there.
This check makes that promise enforceable: it runs on every push and pull request (GitHub Actions,
.github/workflows/checks.yml), and a new address anywhere in the app's code fails the build until it is
added to network-hosts.txt, which means it's visible in the commit and reviewed.

It reads the app's own code (src-tauri/src, ui). Addresses built at run time from data a store sends
back (image and download servers in API answers) can't be seen here; network-hosts.txt says which
services do that.

    scripts/check-network.py           check (exit 1 if something isn't listed)
    scripts/check-network.py --list    print every address found and where
"""
import re
import sys
from pathlib import Path

root = Path(__file__).resolve().parent.parent
SOURCES = [
    *(root / "src-tauri/src").rglob("*.rs"), *(root / "ui").glob("*.js"), *(root / "ui").glob("*.html"),
    # Lodestone Comms (the plug-in, its client and the self-hosted server)
    *(root / "comms").glob("*/src/**/*.rs"), *(root / "comms/app/ui").glob("*.js"), *(root / "comms/app/ui").glob("*.html"),
]
URL = re.compile(r"""\b(?:https?|wss?)://([A-Za-z0-9{}$._*-]+)""")


def normalize(host: str) -> str | None:
    host = host.lower().rstrip(".")
    # Parts filled in at run time: "{region}.version.battle.net" -> "*.version.battle.net".
    host = re.sub(r"\$?\{[^}]*\}", "*", host)
    if "." not in host or host.startswith("*") and host.count(".") < 2:
        return None  # placeholders in tests and examples ("https://img/...")
    if host.endswith((".invalid", ".example", ".test")):
        return None  # reserved names that can never be reached (RFC 2606), used in tests
    return host


def found() -> dict[str, list[str]]:
    hosts: dict[str, list[str]] = {}
    for f in SOURCES:
        for n, line in enumerate(f.read_text(errors="replace").splitlines(), 1):
            for m in URL.finditer(line):
                h = normalize(m.group(1))
                if h:
                    hosts.setdefault(h, []).append(f"{f.relative_to(root)}:{n}")
    return hosts


def allowed() -> set[str]:
    out = set()
    for line in (root / "network-hosts.txt").read_text().splitlines():
        line = line.split("#", 1)[0].strip()
        if line:
            out.add(line.lower())
    return out


def main() -> int:
    hosts = found()
    if "--list" in sys.argv:
        for h in sorted(hosts):
            print(f"{h:48} {hosts[h][0]}" + (f" (+{len(hosts[h]) - 1} more)" if len(hosts[h]) > 1 else ""))
        return 0
    ok = allowed()
    missing = {h: w for h, w in hosts.items() if h not in ok}
    unused = sorted(ok - set(hosts))
    if missing:
        print("These addresses are in the code but not in network-hosts.txt.")
        print("If the change is intended, add each one there (and to the README's Security and privacy")
        print("section if it's a new service), so it's visible and reviewed:\n")
        for h, where in sorted(missing.items()):
            print(f"  {h:44} {', '.join(where[:3])}")
        return 1
    print(f"OK: all {len(hosts)} addresses in the code are listed in network-hosts.txt.")
    if unused:
        print("Listed but no longer in the code (can be removed): " + ", ".join(unused))
    return 0


if __name__ == "__main__":
    sys.exit(main())
