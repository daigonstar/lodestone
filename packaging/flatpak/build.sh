#!/usr/bin/env bash
# Build the Lodestone Flatpak, install it for the current user and write a lodestone.flatpak bundle.
# Needs: flatpak install --user flathub org.flatpak.Builder org.gnome.Sdk//50 org.freedesktop.Sdk.Extension.rust-stable//25.08
set -euo pipefail
cd "$(dirname "$0")"
builder=(flatpak run org.flatpak.Builder)
command -v flatpak-builder >/dev/null && builder=(flatpak-builder)
"${builder[@]}" --user --install --force-clean --repo=repo build-dir dev.lodestone.launcher.yml
flatpak build-bundle repo lodestone.flatpak dev.lodestone.launcher
echo "Installed dev.lodestone.launcher; bundle: $(pwd)/lodestone.flatpak"
