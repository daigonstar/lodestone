# Lodestone

One library for every game on this Linux box. It reads each launcher's own data files and launches games
headless, so Steam stays in the tray and Heroic / Lutris / ATLauncher windows never show up.

| Store | Discovered from | Launched with (hidden) |
|---|---|---|
| Steam (native or Flatpak) | `libraryfolders.vdf`, `appmanifest_*.acf` | `steam -silent steam://rungameid/<id>` |
| Epic, GOG, Amazon | Heroic's `store_cache` + installed JSON | `heroic --no-gui heroic://launch?appName=…&runner=…` |
| Battle.net, ArenaNet (GW2), EA, Ubisoft, anything in Lutris | Lutris `pga.db` | `lutris lutris:rungameid/<id>` |
| Minecraft | ATLauncher / Prism instance folders | `--launch <instance> --close-launcher` / `-l <instance>` |
| Anything else | `~/.config/lodestone/games.json` (or **+ Add game**) | your command via `sh -c` |

All launches run in their own process group with stdin/stdout detached; output goes to
`~/.local/share/lodestone/logs/<game>.log` (⋯ → View launch log). Running state is tracked by watching
`/proc` (Steam's `AppId=<id>` argument, or the game's install folder), which also records playtime in
`~/.local/share/lodestone/state.json`.

## Steam: install, uninstall, Workshop

- Turn on **Not installed** to see every game you own on Steam (read offline from Steam's own `appinfo.vdf`
  cache and `localconfig.vdf`, no API key).
- **Steam stays in the background.** Lodestone starts Steam silently (tray only) and installs, updates and
  uninstalls Steam games itself, with progress in Lodestone's downloads panel, including updates Steam starts on its own.
  Steam only reads its app manifests at startup, so Lodestone stops Steam for a few seconds, writes the change
  (a new manifest marked "update required" to install, a cleared update schedule to update now, the game's files
  removed to uninstall; the Proton prefix with saves is kept) and restarts it with `-silent`. Progress comes from
  Steam's `logs/content_log.txt` and the manifests. The store browser's **Install** buttons work the same way.
  While a Steam game is running Lodestone never restarts Steam and uses Steam's own dialogs instead.
  Turn it off in Settings → Steam to always use Steam's dialogs.
- Games with Workshop support get a **Workshop** button: your installed items (titles and previews from Steam's
  public API), **Browse Workshop** in a Lodestone window (sign in to Steam Community once to subscribe), or open it in Steam.

## Home, details and artwork

- **Home** dashboard: what's installed, pending updates (with each game's latest patch notes and an Update
  button), games you played recently, and a news feed from your games (Steam announcements, filterable to
  patch notes). News and patch notes open in a Lodestone window.
- Selecting a game shows its **description, developer, publisher, release date, Metacritic score, features,
  screenshots and latest news** under the header. Data comes from Steam's public store API. Epic, GOG and
  other games use the Steam game with the same title, or the one you pick.
- **ProtonDB ratings** (Platinum to Borked) show on covers, in the list view and in the details panel, which links
  to the game's ProtonDB page. Steam games with a Linux version show **Native** instead. Ratings are cached for a day.
- **Library layouts:** covers, compact covers, wide banners, or a list with genres, time played, last played and size.
- **Edit details & artwork** (⋯ menu or the details panel): change the title, genres or series, pick which Steam
  game it is, and choose covers and banners from Steam's store art or SteamGridDB. SteamGridDB needs a free
  API key, set in Settings → Artwork. Chosen images are saved in `~/.local/share/lodestone/art`; your edits are kept in `meta.json`.
- **Missing artwork:** Lodestone checks which covers don't exist and lists those games. **Find artwork** fills in
  what it can. Steam games with no portrait cover get one built from their wide store header.
- Epic's library also lists Unreal Engine assets and tools you own. Lodestone only shows the ones Epic files under games.

## Friends

- **👥 Friends** (top bar) opens your friends from Steam and Epic, with Steam friends grouped by in game, online
  and offline. The top-bar count shows how many are online. If a friend is playing a game in your library, click it to open that game.
  Profiles and **Steam chat** open in a Lodestone window (sign in to Steam Community once).
- The **Home** dashboard shows who's playing now. A game's details list **friends who own it** and their hours,
  and the heading changes to **Friends playing now** while any of them are playing it.
- **Steam** uses your Web API key: friends list, status, current game, and each friend's library (cached for a day).
  Your Friends List must be public in Steam's privacy settings. **Epic** uses Lodestone's Epic sign-in and lists names only,
  because Epic shares online status only over its chat connection. GOG and Battle.net have no public friends API.

## Minecraft hub

Click **Minecraft** in the sidebar, or start straight into it with `lodestone --view minecraft`
(handy as its own desktop shortcut). ATLauncher / Prism still handle Microsoft sign-in and running the game; Lodestone manages content:

- **Mods / Resource packs / Shaders**, per instance, matched to its Minecraft version and loader.
- **Installed**: files are identified on Modrinth by hash (name, icon, version). Enable/disable (ATLauncher's
  `disabledmods/` or Prism's `.disabled`), move to trash, **Check for updates**, update one or all.
- **Browse**: search Modrinth (no key) or CurseForge (free personal key in ⚙ Settings) and install with required
  dependencies. Downloads are checksum-verified.
- ATLauncher instances: new and removed files are also written to `instance.json` (backup kept as
  `instance.json.lodestone-backup`), so close ATLauncher while changing mods.

## Run

```bash
cd src-tauri && cargo tauri dev        # from the repo root: cargo tauri dev
cargo tauri build                      # AppImage + .deb in src-tauri/target/release/bundle
```

See what's detected without the UI:

```bash
cd src-tauri && cargo test print_library -- --ignored --nocapture
```

## Epic Games and GOG without Heroic

**👤 Accounts → Sign in** opens the store's official sign-in page in a Lodestone window; Lodestone catches the
sign-in code and hands it to its own copies of [legendary](https://github.com/derrod/legendary) (Epic) and
[gogdl](https://github.com/Heroic-Games-Launcher/heroic-gogdl) (GOG), kept in `~/.local/share/lodestone/tools`.
(Or **Use Heroic's sign-in** to move Heroic's session over; Heroic then has to sign in again.)

- Your whole library shows up (Rescan also refreshes it from the stores).
- **Install** downloads with progress at the bottom of the window, into `~/Games/Epic` / `~/Games/GOG`
  (change in ⚙ Settings → Games). Cancel and re-install resumes.
- Games launch through Proton (umu) in their own prefix, with GE-Proton's per-store fixes.
- Before a GOG game first starts, Lodestone does what GOG Galaxy does after an install: runs the game's
  `goggame-<id>.script` (registry keys, save folders, first-run files, small fix-up tools) and puts GOG's
  support files in the game folder. Older games such as SimCity 4 don't start without this.
- **Uninstall**: Epic deletes the files (legendary); GOG moves the folder to the trash.
- When Heroic is still installed, a game installed there keeps launching through Heroic until you install it via Lodestone.

## Proton without Steam

Lodestone keeps its own copy of [umu-launcher](https://github.com/Open-Wine-Components/umu-launcher)
in `~/.local/share/lodestone/tools` (⚙ Settings → Compatibility → Set up / Update) and runs Windows programs
through Proton in per-app prefixes under `~/.local/share/lodestone/prefixes/`. It uses the newest GE-Proton
it finds (ProtonUp-Qt / Steam folders), or one you pick, or downloads the latest GE-Proton automatically.

## Battle.net and Guild Wars 2

Open **Battle.net** in the sidebar → **Set up Battle.net**: Lodestone downloads Blizzard's official installer and
installs it into its own Proton prefix (with GE-Proton's Battle.net fixes via `STORE=battlenet`). Sign in and install
games in Battle.net; after a Rescan each game (found via its `.build.info`) launches from Lodestone with
`Battle.net.exe --exec="launch <code>"`. **ArenaNet → Install Guild Wars 2** does the same with ArenaNet's installer.

**+ Add game → "Windows game: run it with Proton"** runs any `.exe` in its own prefix.

### Older notes

Install them through Lutris (search "Battle.net" / "Guild Wars 2" in Lutris and use the community installer).
They appear here automatically, tagged as Battle.net / ArenaNet. Or add any Wine game by hand with
**+ Add game**, e.g. `umu-run "/home/you/Games/gw2/Gw2-64.exe"`.

## Keys

`/` search · arrows move · `Enter` play · `F` favorite · `Ctrl+R` rescan
