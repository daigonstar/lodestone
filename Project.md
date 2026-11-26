# Lodestone project status

What needs doing to get Lodestone to users, and where each item stands. Update this file whenever an
item is finished or partly done.

Last updated: 2026-11-26 (Beacon mobile plan written)

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
| App icon for Beacon (`beacon/branding/beacon-icon.svg`) | Drawn and approved by Robb. Not yet installed anywhere (no window or Flatpak icon files made yet) |
| Matching icons for Lodestone and Kiln | Not started |
| Redo the internal icons (camera, streaming, etc.) in one style | Done for Beacon: mystic line icon set in the interface (`beacon/branding/icons.svg`, inlined in `index.html`). Checked in a headless browser on Linux only; call bar not seen live. Lodestone's own icons still emoji |
| Move the layout and look away from Discord | Layout A (top bar of communities, channel list left, centred chat, floating call pill, members pull-out, gem avatars) chosen by Robb and built in `beacon/app/ui`. Checked in a headless browser with a local server (chat, tabs, members, call pill with injected markup); the call stage was rendered with fake participants (speaking card gets a thin violet and cyan light travelling slowly round the outside of the card in place of the outline, nothing drawn over camera pictures, spin stops with reduced-motion; thin banner inside the call view with who is sharing and a Watch button) but a real call was not run |
| Polish the welcome screen | Done in `beacon/app/ui`: slogan "Find your people. Keep it private." (Robb's pick), Beacon mark and glow, joined communities as cards on the home page, join and create cards, tips folded away, one-line test-build note. Checked only in a headless browser, not on Windows |
| Community banners | Done: owners and admins can set a banner (PNG, JPEG, GIF or WebP, up to 512 KiB) in community settings; it shows on the community's home-page card, with a colour fallback. New server route `/v1/communities/{id}/banner` (tested), documented in PROTOCOL.md. Existing servers add the column on start-up |
| Guided server setup | Done: "Set up your own server" opens a step-by-step page (this device or another device, who will use it, how to start it, an address check, admin account, first community). "Just me, to try it" has a **Start it for me** button that runs a Beacon server inside the app on 127.0.0.1:8080 only (data in Beacon's own folder `local-server`, voice off, comes back up when Beacon starts). Friends-over-the-internet and other-device paths show the Docker steps from the README with copy buttons; they were not run here. Tested end to end against the in-app server, including a restart |
| PR #7 (plug-in installer) still uses `comms`, `comms-v` and the old Flatpak id | Needs merging with this rename before either lands |
| Existing `comms-v*` draft releases on GitHub | Would be re-published as `beacon-v*` |

## Beacon mobile apps (Android and iOS)

Robb wants Beacon on both stores, with the phone as a paired device and an authenticator for new PCs.
Plan written 2026-11-26: `beacon/MOBILE-PLAN.md` (approaches compared, store rules, AGENTS.md conflicts, phases).
Recommendation: Tauri 2 mobile reusing the Rust core and web UI, native Kotlin/Swift plug-ins where needed.
**Decided by Robb 2026-11-26: Android first, free route (GitHub releases and F-Droid); UK users to start. iOS (US$99 a year, app removed if it lapses) and Google Play (US$25) are optional later.**

| Item | Status |
| --- | --- |
| Plan and comparison | Done, waiting for Robb's choice. Nothing built |
| Apple (US$99/yr) and Google Play (US$25) developer accounts | Not needed for Android first; decide later |
| Decide push notifications (no push first, or a publisher relay that needs an AGENTS.md change) | Waiting on Robb |
| Spike: Beacon client on Android and iOS | Not started |
| Phase 1: Android pairing and chat, history copy, report/block, account deletion | Not started |
| Phase 2: F-Droid listing, notifications | Not started |

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
