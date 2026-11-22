#!/usr/bin/env python3
"""Write cargo-sources.json for an offline Flatpak build (Flathub builds without network access).

Every crate in src-tauri/Cargo.lock becomes a flatpak-builder source: the .crate archive from
crates.io (checked against the lock file's sha256), unpacked into cargo/vendor/<name>-<version>,
plus the .cargo-checksum.json cargo expects there. The last source is a cargo config that points
crates.io at that vendor folder.

Run it again whenever Cargo.lock changes:  packaging/flatpak/cargo-sources.py
"""
import json
import sys
import tomllib
from pathlib import Path

here = Path(__file__).resolve().parent
lock = tomllib.loads((here / "../../src-tauri/Cargo.lock").read_text())

sources = []
skipped = []
for pkg in lock["package"]:
    src = pkg.get("source", "")
    if not src:
        continue  # the app itself
    if not src.startswith("registry+https://github.com/rust-lang/crates.io-index"):
        skipped.append(f"{pkg['name']} ({src})")
        continue
    name, version, checksum = pkg["name"], pkg["version"], pkg["checksum"]
    dest = f"cargo/vendor/{name}-{version}"
    sources.append({
        "type": "archive",
        "archive-type": "tar-gzip",
        "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
        "sha256": checksum,
        "dest": dest,
    })
    sources.append({
        "type": "inline",
        "contents": json.dumps({"package": checksum, "files": {}}),
        "dest": dest,
        "dest-filename": ".cargo-checksum.json",
    })

if skipped:
    sys.exit("Crates not from crates.io need adding by hand:\n  " + "\n  ".join(skipped))

sources.append({
    "type": "inline",
    "contents": '[source.vendored-sources]\ndirectory = "cargo/vendor"\n\n[source.crates-io]\nreplace-with = "vendored-sources"\n',
    "dest": "cargo",
    "dest-filename": "config.toml",
})

out = here / "cargo-sources.json"
out.write_text(json.dumps(sources, indent=1) + "\n")
print(f"{out}: {len(sources) // 2} crates")
