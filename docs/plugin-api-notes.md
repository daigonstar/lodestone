# Notes for the plug-in author write-up

Working notes on the plug-in API as it stands on the `comms` branch (not a finished guide; roadmap item 2
turns this into one). The formal draft of the bridge is `comms/PLUGIN.md`.

## Two kinds of plug-in

| | Embedded (Comms) | Companion (Kiln) |
|---|---|---|
| What it is | Its own program (Flatpak on Linux, zip in `%LOCALAPPDATA%\Lodestone\Comms` on Windows) | An app the user installs themselves, with its own window |
| How Lodestone starts it | `<program> --lodestone-bridge`, stdin/stdout joined to Lodestone | Only for one call at a time, with arguments (`kiln --games`, `--play <id>` ...) |
| Channel | JSON lines: `hello`, `request`/`response`, `event` | The program's command line; JSON on stdout for read-only calls |
| Interface | A page on `127.0.0.1` shown in a frame in Lodestone | The app's own window |
| What Lodestone grants | Methods per approved permission (`friends:read`, `chat:open`, `game:read`) | Nothing. Lodestone only calls it |
| Switch | Start/stop, revoke permissions | On/off (`plugins_disabled` in settings) |

## Where it lives in the code

- `src-tauri/src/plugins.rs`: the registry. `list()` returns one `PluginInfo` per plug-in (id, name,
  description, `kind`, `installed`, `enabled`, `uses`, `note`, `permissions`, `install_hint`).
  `enabled(id)` / `set_enabled(id, on)` for companions. `describe()` holds the plain-words text for each
  bridge permission.
- `src-tauri/src/kiln.rs`: the companion. `command()` is the single gate: it returns `None` when the
  plug-in is off or Kiln isn't found, so every feature (tags, Modded view, Play modded, Set up mods)
  disappears together. `installed()` ignores the switch.
- `src-tauri/src/comms_install.rs`: installing an embedded plug-in from official releases
  (tag `comms-v<version>`, checksum required).
- `ui/app.js` `renderPluginsPage()`: draws a card per plug-in from `PluginInfo`; sidebar entries only for
  plug-ins that have a page (`PLUGIN_VIEW`).

## What a companion needs to offer

Read-only JSON commands (never start the GUI): `--games` (id, name, Steam app ids, installed, managed,
mod count), `--status <id>`. Action commands: `--game <id>` (open on a game, forwarding to a running
instance), `--play <id>`, `--play-vanilla <id>`. Unknown fields are ignored and missing optional ones
tolerated (older Kiln versions lack `mods`), so a companion can add fields without breaking Lodestone.

## Installing and updating (`plugin_sources.rs`)

- **Official** plug-ins are a fixed list in the code (`official()`: id, repo, release tag prefix). A clean
  install knows nothing else.
- **Third-party**: you type `owner/repo` on the Plug-ins page; it's stored only in your settings
  (`plugin_repos`), like a Comms server address. GitHub only, name checked (`parse_repo`).
- A release is usable when it isn't a draft or pre-release, has the tag prefix, and has a file ending
  `-linux-x86_64.flatpak` (Linux) or `-windows-x86_64.zip` (Windows). The file must be downloaded from
  `https://github.com/<repo>/releases/download/` and carry GitHub's `sha256:` digest, which is checked.
- Optional `lodestone-plugin.json` in the release (≤16 KB): `name`, `description`, `kind`
  (`embedded` or `companion`), `permissions`. It's only shown to you before installing.
- Checks happen only when the Plug-ins page is opened; installs only on a button press.
- Linux installs the Flatpak for your user. Windows unpacks the zip to
  `%LOCALAPPDATA%\Lodestone\Plugins\<id>\<version>` with `current.txt` naming the version in use (Comms
  keeps its older `Comms\` layout).
- Not built yet: running third-party *embedded* plug-ins (the host in `plugins.rs` handles only Comms),
  and release signing. Third-party plug-ins other than Flatpaks are not sandboxed.

## Windows

- Locating a program: on Linux, PATH then the `.desktop` entry; on Windows there are no `.desktop` files,
  so it's `*.exe` on PATH and fixed folders under `%LOCALAPPDATA%`. `host::which` doesn't add `.exe`.
- Kiln itself has no Windows build, so on Windows the card says so (`note`).

## Open questions for the write-up

- Release signing (public key built into Lodestone) to protect against a taken-over repository.
- Should a companion get a bridge (events, permissions) so it can show state inside Lodestone?
- A manifest file (`plugin.json`) instead of ids hard-coded in `plugins.rs`, so third parties can add
  plug-ins without changing Lodestone. Today adding one means editing `plugins.rs` and `app.js`.
