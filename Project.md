# Lodestone project status

What needs doing to get Lodestone to users, and where each item stands. Update this file whenever an
item is finished or partly done.

Last updated: 2026-11-26 (Kiln plug-in, plug-in installer)

## Current focus

**AUR release: paused.** The AUR account can't be created at the moment, so nothing can be published
to the AUR. The packaging work is done and kept up to date in the repository, so it can be picked up
again as soon as an account exists. Meanwhile, the release channels below that don't need the AUR
carry on.

## Roadmap

In priority order, set by Robb on 2026-11-26. A longer feature list lives on Robb's home PC and isn't
tracked here.

| # | Item | Status |
| --- | --- | --- |
| 1 | Windows version at the same level as the Linux version | Not started |
| 2 | Write-up on how other people can make plugins for Lodestone | Not started |
| 3 | Convert Kiln into a plugin, released as 0.3.0 (target: mid December 2026) | **Partly done** on the `comms` branch (not merged to `main`): see below |

## Kiln as a plugin (roadmap item 3)

Work stays on the `comms` branch until 0.3.0. Kiln is a *companion* plugin: it keeps its own Qt window, and
Lodestone talks to it only through its command line, so it needs no bridge permissions. Notes on the
shape of the plugin API are in `docs/plugin-api-notes.md`.

| Item | Status |
| --- | --- |
| Kiln listed on the Plug-ins page with an on/off switch (off hides all Kiln tags and launching) | Done, builds and unit test pass on Linux; UI not clicked through yet |
| Windows-aware Kiln lookup (replaces the Linux-only `.desktop` search on Windows: `kiln.exe` on PATH, `%LOCALAPPDATA%\Lodestone\Kiln`, `%LOCALAPPDATA%\Programs\Kiln`) | Done in code, not built or run on Windows |
| Kiln itself running on Windows | **Not started**: Kiln is Linux-only (Proton prefixes, Steam's Linux folders, Qt). Needs a port before the Windows lookup finds anything |
| One-click install/update from official GitHub releases (checksum-verified), shared by Comms and Kiln; users can add their own GitHub repos for third-party plug-ins | Done in code, unit-tested; page smoke-tested with a stubbed backend only, never against real releases. Kiln needs to publish a release (`…-linux-x86_64.flatpak`, app id `io.github.daigonstar.Kiln`) before it can install |
| Running third-party embedded plug-ins; release signing | Not started |
| Kiln speaks the Lodestone bridge (`--lodestone-bridge`) and shows its page in Lodestone | Not started: decide whether it's wanted (needs a web UI) |

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
