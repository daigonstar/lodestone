#!/usr/bin/env bash
# Build the Lodestone Flatpak from this checkout (offline, like Flathub), install it for the current
# user and write a lodestone.flatpak bundle.
# Needs: flatpak install --user flathub org.flatpak.Builder org.gnome.Sdk//51 org.freedesktop.Sdk.Extension.rust-stable//26.08
set -euo pipefail
cd "$(dirname "$0")"
app=io.github.daigonstar.Lodestone

# Same manifest as Flathub's, with the GitHub tag swapped for this folder.
python3 -I - "$app.yml" local.yml <<'EOF'
import re, sys
text = open(sys.argv[1]).read()
local = """      - type: dir
        path: ../..
        skip:
          - src-tauri/target
          - packaging/flatpak/.flatpak-builder
          - packaging/flatpak/build-dir
          - packaging/flatpak/repo
          - .git
"""
text, n = re.subn(r"(# app-source-begin[^\n]*\n).*?(\s*# app-source-end)", lambda m: m.group(1) + local + m.group(2).lstrip("\n"), text, flags=re.S)
assert n == 1, "app source markers not found"
open(sys.argv[2], "w").write(text)
EOF

# Keep the crate list in step with Cargo.lock.
python3 -I cargo-sources.py

builder=(flatpak run org.flatpak.Builder)
command -v flatpak-builder >/dev/null && builder=(flatpak-builder)
"${builder[@]}" --user --install --force-clean --repo=repo build-dir local.yml
flatpak build-bundle repo lodestone.flatpak "$app"
echo "Installed $app; bundle: $(pwd)/lodestone.flatpak"
