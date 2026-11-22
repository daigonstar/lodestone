# Making a release

1. Bump `version` in `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml` (then build once so
   `Cargo.lock` follows), and add a `<release>` entry to
   `packaging/flatpak/io.github.daigonstar.Lodestone.metainfo.xml`. Commit and push.
2. Tag it and push the tag (`git tag 0.1.2 && git push origin refs/tags/0.1.2`). GitHub Actions
   (`.github/workflows/release.yml`) builds the AppImage, `.deb` and binary tarball on Ubuntu 22.04,
   creates the GitHub release and attaches them.
3. When that's done, run `scripts/release.sh <version> --upload`: it builds the Flatpak bundle from the
   tag, attaches it to the release, and updates both AUR packages in `packaging/aur/` (version,
   checksums, `.SRCINFO`). Commit those.
4. Copy `PKGBUILD`, `.SRCINFO` and `lodestone.desktop` from each `packaging/aur/<package>` folder into its
   AUR repository and push.
5. For Flathub: point the `tag` and `commit` in `packaging/flatpak/io.github.daigonstar.Lodestone.yml`
   at the new tag, and if `Cargo.lock` changed, run `packaging/flatpak/cargo-sources.py`.
