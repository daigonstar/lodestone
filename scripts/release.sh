#!/usr/bin/env bash
# The local half of a release (GitHub Actions builds the AppImage, .deb and binary tarball when the tag
# is pushed; see .github/workflows/release.yml):
#   1. builds the Flatpak bundle from the tag            -> dist/<v>/lodestone-<v>.flatpak
#   2. updates both AUR packages in packaging/aur/        (version, checksums, .SRCINFO)
#      lodestone-bin needs the workflow's tarball on the GitHub release first.
#
#   scripts/release.sh 0.1.0            build the bundle and update the AUR files
#   scripts/release.sh 0.1.0 --upload   ...and attach the Flatpak bundle to the GitHub release
set -euo pipefail
ver=${1:?usage: scripts/release.sh VERSION [--upload]}
upload=${2:-}
root="$(cd "$(dirname "$0")/.." && pwd)"
repo=daigonstar/lodestone
tag=$ver
out="$root/dist/$ver"
work="$root/dist/.build-$ver"

git -C "$root" rev-parse -q --verify "refs/tags/$tag" >/dev/null || { echo "No tag $tag" >&2; exit 1; }
conf_ver=$(git -C "$root" show "$tag:src-tauri/tauri.conf.json" | python3 -I -c 'import json,sys;print(json.load(sys.stdin)["version"])')
[ "$conf_ver" = "$ver" ] || { echo "Tag $tag has version $conf_ver in tauri.conf.json, not $ver" >&2; exit 1; }

rm -rf "$work" "$out"
mkdir -p "$out"
git -C "$root" worktree prune
git -C "$root" worktree add --detach "$work" "$tag" >/dev/null
trap 'git -C "$root" worktree remove --force "$work" >/dev/null 2>&1 || true' EXIT

echo "== Flatpak bundle from $tag"
"$work/packaging/flatpak/build.sh" >"$out/flatpak.log" 2>&1 || { tail -30 "$out/flatpak.log" >&2; exit 1; }
cp "$work/packaging/flatpak/lodestone.flatpak" "$out/lodestone-$ver.flatpak"
rm -f "$out/flatpak.log"
(cd "$out" && sha256sum -- "lodestone-$ver.flatpak" >SHA256SUMS-flatpak)

echo "== AUR: lodestone (builds from source)"
d="$root/packaging/aur/lodestone"
sed -i "s/^pkgver=.*/pkgver=$ver/; s/^pkgrel=.*/pkgrel=1/" "$d/PKGBUILD"
sums=$(cd "$d" && makepkg -g 2>/dev/null)
rm -f "$d/lodestone-$ver.tar.gz"
python3 -I - "$d/PKGBUILD" "$sums" <<'PY'
import re, sys
p, sums = sys.argv[1], sys.argv[2].strip()
s = open(p).read()
open(p, "w").write(re.sub(r"sha256sums=\([^)]*\)", sums, s, flags=re.S))
PY
(cd "$d" && makepkg --printsrcinfo >.SRCINFO)

echo "== AUR: lodestone-bin (prebuilt, from the GitHub release)"
d="$root/packaging/aur/lodestone-bin"
sed -i "s/^pkgver=.*/pkgver=$ver/; s/^pkgrel=.*/pkgrel=1/" "$d/PKGBUILD"
if sums=$(cd "$d" && makepkg -g 2>/dev/null); then
  rm -f "$d"/lodestone-"$ver"-x86_64.tar.gz
  python3 -I - "$d/PKGBUILD" "$sums" <<'PY'
import re, sys
p, sums = sys.argv[1], sys.argv[2].strip()
s = open(p).read()
open(p, "w").write(re.sub(r"sha256sums=\([^)]*\)", sums, s, flags=re.S))
PY
  (cd "$d" && makepkg --printsrcinfo >.SRCINFO)
else
  echo "   The release has no lodestone-$ver-x86_64.tar.gz yet: wait for the workflow, then run this again." >&2
fi

if [ "$upload" = "--upload" ]; then
  echo "== Attaching the Flatpak bundle to the GitHub release $tag"
  gh release upload "$tag" "$out/lodestone-$ver.flatpak" "$out/SHA256SUMS-flatpak" --clobber -R "$repo"
fi
echo "Done: $out"
