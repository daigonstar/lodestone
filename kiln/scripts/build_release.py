#!/usr/bin/env python3
"""Build the files for a Kiln release into dist/ (run from anywhere):

    kiln-<version>-linux-any.zip   Kiln's Python package, nothing else. Lodestone installs this (Plug-ins
                                   page) and runs it with the system's Python and PySide6.
    lodestone-plugin.json          what the plug-in is, read by Lodestone before installing

The version comes from kiln/kiln/__init__.py; pass --tag kiln-v<version> to check a release tag matches it.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent  # the kiln/ folder of the Lodestone repository


def version() -> str:
    m = re.search(r'^__version__ = "([0-9][0-9A-Za-z.\-]*)"', (ROOT / "kiln" / "__init__.py").read_text(), re.M)
    if not m:
        sys.exit("can't find __version__ in kiln/kiln/__init__.py")
    return m.group(1)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--tag", help="the release tag; must be v<version>")
    ap.add_argument("--out", default=str(ROOT / "dist"))
    args = ap.parse_args()
    v = version()
    if args.tag and args.tag != f"kiln-v{v}":
        sys.exit(f"tag {args.tag} doesn't match kiln/kiln/__init__.py version {v} (expected kiln-v{v})")
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    zip_path = out / f"kiln-{v}-linux-any.zip"
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as z:
        for f in sorted((ROOT / "kiln").rglob("*")):
            if f.is_file() and "__pycache__" not in f.parts and f.suffix != ".pyc":
                z.write(f, f.relative_to(ROOT).as_posix())
    # Kiln has no permissions to ask Lodestone for: Lodestone only runs its command line.
    (out / "lodestone-plugin.json").write_text(json.dumps({
        "name": "Kiln",
        "description": "Mod manager for Steam games: Nexus mods, load order, profiles and crash checks.",
        "kind": "companion",
        "permissions": [],
    }, indent=2) + "\n")
    print(zip_path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
