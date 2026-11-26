# Lodestone project status

What needs doing to get Lodestone to users, and where each item stands. Update this file whenever an
item is finished or partly done.

Last updated: 2026-11-26

## Current focus

**AUR release: paused.** The AUR account can't be created at the moment, so nothing can be published
to the AUR. The packaging work is done and kept up to date in the repository, so it can be picked up
again as soon as an account exists. Meanwhile, the release channels below that don't need the AUR
carry on.

## Releases

| Item | Status |
| --- | --- |
| GitHub releases (AppImage, `.deb`, binary tarball, Flatpak bundle) | Done, latest is 0.2.0 (2026-11-25) |
| Security review fixes shipped in a release | Done, in 0.1.2 and later |
| Unreleased changes on `main` | 7 commits since 0.2.0 |

## AUR (paused)

Paused on 2026-11-26: the AUR account can't be created yet.

| Item | Status |
| --- | --- |
| `packaging/aur/lodestone` (builds from source) PKGBUILD and `.SRCINFO` | Done, at 0.2.0 |
| `packaging/aur/lodestone-bin` (prebuilt) PKGBUILD and `.SRCINFO` | Done, at 0.2.0 |
| `scripts/release.sh` updates both AUR packages each release | Done |
| Test-build both packages on Arch (clean chroot, `namcap`) | Not done, needs an Arch machine |
| Create an AUR account | **Blocked**, can't create one yet |
| Check the names `lodestone` and `lodestone-bin` are free on the AUR | Not done, waiting on the account |
| Create the AUR repositories and push PKGBUILD and `.SRCINFO` | Paused, waiting on the account |
| README's "Install" table no longer promises `yay -S lodestone` | Done, says the AUR packages aren't published yet |

To resume: create the account, add an SSH key to it, then follow step 4 of `RELEASING.md`.

## Other channels that can go ahead meanwhile

- **GitHub releases:** already working. Arch users can use the AppImage or the binary tarball, or
  build `packaging/aur/lodestone` themselves with `makepkg -si` from a clone of this repository.
- **Flathub:** the manifest, metainfo and screenshots in `packaging/flatpak/` already exist and point
  at 0.2.0. Submitting it needs a GitHub account only. Flathub reviewers may question the host access
  the Flatpak needs (`--filesystem=host`, `--talk-name=org.freedesktop.Flatpak`).
- **Debian and Ubuntu:** the `.deb` is built each release. A PPA or other apt repository could come
  later.
