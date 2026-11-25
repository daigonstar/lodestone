#!/usr/bin/env bash
# Install Lodestone for the current user: binary in ~/.local/bin, icons in the hicolor theme,
# and a launcher entry (with "Minecraft" and "Modded games" quick actions).
# Run after `cargo tauri build --no-bundle`. Re-run any time to update.
# `--binary-only` installs just the program (for testing alongside the Flatpak).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
bin="$root/src-tauri/target/release/lodestone"
svg="$root/assets/lodestone.svg"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
dest_bin="$HOME/.local/bin/lodestone"

[ -x "$bin" ] || { echo "Build first: (cd \"$root\" && cargo tauri build --no-bundle)" >&2; exit 1; }

install -Dm755 "$bin" "$dest_bin"

# --binary-only: just the program, for testing next to an installed Flatpak (no second menu entry).
if [ "${1:-}" = "--binary-only" ]; then
  echo "Installed: $dest_bin (binary only, no launcher entry)."
  exit 0
fi

for s in 16 24 32 48 64 128 256 512; do
  out="$data/icons/hicolor/${s}x${s}/apps/lodestone.png"
  mkdir -p "$(dirname "$out")"
  rsvg-convert -w "$s" -h "$s" "$svg" -o "$out"
done
install -Dm644 "$svg" "$data/icons/hicolor/scalable/apps/lodestone.svg"

install -Dm644 /dev/stdin "$data/applications/lodestone.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Lodestone
GenericName=Game Launcher
Comment=All your games in one library: Steam, Epic, GOG, Battle.net, Minecraft and more
Exec=$dest_bin
Icon=lodestone
Terminal=false
Categories=Game;
Keywords=games;launcher;steam;epic;gog;battle.net;minecraft;proton;mods;
StartupNotify=true
StartupWMClass=lodestone
Actions=minecraft;modded;

[Desktop Action minecraft]
Name=Minecraft
Exec=$dest_bin --view minecraft

[Desktop Action modded]
Name=Modded games
Exec=$dest_bin --view modded
EOF

# A per-user hicolor folder usually has no index.theme, so only refresh a cache that already exists.
if [ -f "$data/icons/hicolor/index.theme" ] && command -v gtk-update-icon-cache >/dev/null; then
  gtk-update-icon-cache -q -f "$data/icons/hicolor" || true
fi
command -v update-desktop-database >/dev/null && update-desktop-database -q "$data/applications" || true
echo "Installed: $dest_bin, launcher entry and icons."
