#!/usr/bin/env bash
# Make a Lodestone Comms release: the Linux Flatpak bundle and the Windows zip, named the way
# Lodestone's installer looks for them, with SHA256SUMS, as a DRAFT GitHub release tagged comms-v<version>
# in daigonstar/lodestone. Look it over on GitHub and publish it yourself; nothing is public until then.
#
#   scripts/release-comms.sh <version> <windows zip>
#   e.g. scripts/release-comms.sh 0.2.0 dist/windows/lodestone-comms-windows-test-5.zip
#
# Needs: the gh CLI signed in, the Comms Flatpak build tools (see comms/flatpak/build.sh), and the
# commit being released pushed to GitHub (the release points at it). GitHub publishes a SHA-256 for every
# release file; Lodestone refuses Comms files without one.
set -euo pipefail
cd "$(dirname "$0")/.."
version=${1:?version, e.g. 0.2.0}
winzip=${2:?the Windows zip built in the Windows VM}
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must look like 0.2.0" >&2; exit 1; }
[ -f "$winzip" ] || { echo "no such file: $winzip" >&2; exit 1; }
have=$(sed -n 's/^version = "\(.*\)"/\1/p' comms/app/Cargo.toml | head -1)
[ "$have" = "$version" ] || { echo "comms/app/Cargo.toml says $have, not $version: bump it first" >&2; exit 1; }
[ -z "$(git status --porcelain -- comms src-tauri ui)" ] || { echo "commit your changes first" >&2; exit 1; }

out=dist/comms-release-$version
rm -rf "$out"; mkdir -p "$out"
(cd comms && ./flatpak/build.sh >/dev/null)
flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo comms/flatpak/repo "$out/lodestone-comms-$version-linux-x86_64.flatpak" io.github.daigonstar.Lodestone.Comms
cp "$winzip" "$out/lodestone-comms-$version-windows-x86_64.zip"
(cd "$out" && sha256sum lodestone-comms-* > SHA256SUMS && cat SHA256SUMS)

commit=$(git rev-parse HEAD)
branch=$(git rev-parse --abbrev-ref HEAD)
if ! git branch -r --contains "$commit" | grep -q "origin/"; then
    echo
    echo "Commit $commit isn't on GitHub yet. Publishing a release makes the Comms source public there."
    read -r -p "Push $branch to GitHub now? [y/N] " ok
    [ "$ok" = "y" ] || { echo "Stopped: nothing was pushed or released. Files are in $out." >&2; exit 1; }
    git push origin "$branch"
fi
gh release create "comms-v$version" --draft --prerelease --target "$commit" \
    --title "Lodestone Comms $version" \
    --notes "Lodestone Comms $version (test build). Install it from Lodestone's Plug-ins page, which checks every file against GitHub's published SHA-256. Linux: the Flatpak bundle. Windows: the zip." \
    "$out"/lodestone-comms-* "$out/SHA256SUMS"
echo "Draft release comms-v$version created. Review it on GitHub, then publish it."
