#!/usr/bin/env bash
# Build the Lodestone Comms Flatpak from this checkout (offline, like Flathub) and install it for the
# current user. Needs: org.freedesktop.Sdk//26.08 and org.freedesktop.Sdk.Extension.rust-stable//26.08.
set -euo pipefail
cd "$(dirname "$0")"
app=io.github.daigonstar.Lodestone.Comms
python3 -I ../../packaging/flatpak/cargo-sources.py ../Cargo.lock cargo-sources.json
builder=(flatpak run org.flatpak.Builder)
command -v flatpak-builder >/dev/null && builder=(flatpak-builder)
"${builder[@]}" --user --install --force-clean --repo=repo build-dir "$app.yml"
flatpak build-bundle repo lodestone-comms.flatpak "$app"
echo "Installed $app; bundle: $(pwd)/lodestone-comms.flatpak"
