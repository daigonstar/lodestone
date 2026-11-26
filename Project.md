# Lodestone project status

What needs doing to get Lodestone to users, and where each item stands. Update this file whenever an
item is finished or partly done.

Last updated: 2026-11-26 (Beacon rename done in code)

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
| 1 | Windows version at the same level as the Linux version | In progress: audit done, see "Windows parity" below |
| 2 | Write-up on how other people can make plugins for Lodestone | Not started |
| 3 | Convert Kiln into a plugin, released as 0.3.0 (target: mid December 2026) | Not started |

## Windows parity (roadmap item 1)

The Windows port lives on the `comms` branch. `comms` is **not merged to `main` until 0.3.0 is ready** (Robb, 2026-11-26): it is the next featured version. PR #6 (`comms` into `main`) stays open until then. Audit done 2026-11-26 by reading the
code. Nothing below was run on Windows: this cloud session can only build for Linux, so the Windows
changes are untested and compile-checked only by reading. Linux builds and its 62 tests pass.

Already on the Windows side (`comms` branch): builds and runs, Steam/Epic/GOG launchers via winget and
registry/manifest scanning, emulators via winget, Battle.net/EA/Ubisoft via winget, detached launches,
process list, Comms plug-in install, update checks for installed programs.

| Gap | Priority | Status |
| --- | --- | --- |
| Epic and GOG sign-in, library, install, update and play through legendary and gogdl (Windows downloads) | High | Code added, untested: tools.rs fetches Windows builds, play runs natively. Needs a check that the release files named `legendary.exe` / `gogdl_windows_x86_64.exe` exist (could not look: GitHub API blocked here) |
| Steam in the background (silent install/update/uninstall) | High | Code added, untested: client and running-game detection for Windows |
| Game detection by folder: case and slash differences in Windows paths | High | Done, with a test |
| GOG uninstall uses `gio trash` (Linux only), falls back to deleting | Medium | Works by fallback, should use the Recycle Bin |
| Heroic, Lutris, ATLauncher, Prism: Linux paths only | Medium | Not started. Windows has Heroic and Prism too, with different folders |
| Play time from Lutris/Heroic/Battle.net files: Linux paths | Medium | Not started |
| Kiln integration: looks for a Linux `.desktop` file | Medium | Not started (becomes a plugin in roadmap item 3) |
| Store browser placed with GTK overlay on Linux, plain webview position elsewhere | Medium | Needs a look on Windows |
| `assetProtocol` scope allows only `$HOME`, `/mnt`, `/media`: games on other drives get no artwork | Medium | Not started |
| Packaging: no Windows installer or release build in `release.yml`, no Windows CI check | High | Not started |
| README, install table and wording are Linux-only | Medium | Not started |
| Steam friends, achievements, RetroAchievements, ROM scanning | Low | Shared code, should work, untested on Windows |
| Flatpak, MangoHud, Proton, umu, GOG setup in Wine | n/a | Linux only by design |

Suggested order, all on `comms`: add a Windows build to CI and releases; test the Epic/GOG and Steam
changes on a Windows PC; then the Medium items. Merge `comms` to `main` only for the 0.3.0 release.

## Beacon rename and rebrand (feeds roadmap item 3)

Decided by Robb on 2026-11-26: the Comms plug-in becomes **Lodestone Beacon**. In its words: **server** is
the self-hosted machine/software, **community** is the group people join (was "server"/"space"),
**channel** is unchanged. Test data can start fresh, so there is no migration. It is a full rebrand of the
look too, moving away from Discord's look. Plan and inventory: `beacon/NAMING-PLAN.md`.

| Item | Status |
| --- | --- |
| Plan, inventory, vocabulary approved, name picked | Done |
| Text rename: Comms to Beacon (folder `beacon/`, crates, Flatpak id `io.github.daigonstar.Lodestone.Beacon`, tag prefix `beacon-v`, `BEACON_*` settings, `lodestone-beacon/v1` protocol strings, Lodestone's installer and UI) | Done in code, builds and server/client tests pass on Linux; Windows and Flatpak builds not run |
| Vocabulary change: host to server, server/space to community (UI, API routes, database, docs) | Done, same checks |
| App icon for Beacon and matching icons for the other apps | Not started: direction needs Robb's approval |
| Redo the internal icons (camera, streaming, etc.) in one style | Not started |
| Move the layout and look away from Discord | Not started: direction needs Robb's approval |
| PR #7 (plug-in installer) still uses `comms`, `comms-v` and the old Flatpak id | Needs merging with this rename before either lands |
| Existing `comms-v*` draft releases on GitHub | Would be re-published as `beacon-v*` |

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
