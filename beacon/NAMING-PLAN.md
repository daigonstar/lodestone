# Naming replan for Comms (plan only, no code changed)

Status: **approved by Robb (2026-11-26): vocabulary as below, plug-in name Beacon. Carried out in the same PR; this file is kept as the record, and describes the code as it was before (Comms, host, space).** Robb also said his running server holds nothing valuable and testers can start fresh, so no database migration or Flatpak data copy is needed. He wants the suite of apps to keep fairly similar names. Written 2026-11-26 from a read of the `comms` branch
(and PR #7, the plug-in installer). Nothing has been renamed.

## The problem

Today one word means two things, and the code works around it:

- **Host** = the self-hosted program/machine (one Docker container).
- **Server** = the group people join (the Discord idea). The wire protocol calls it a **space**, to stop
  it clashing with "host".

So a user sees "host" and "server" in the app, and a developer sees "space" in the code and API. Robb's
fix: **server** means the machine/software, **community** means the group. That removes the need for
"space" entirely.

## 1. Inventory: where the current terms live

Counts are lines matching in `comms/`, `src-tauri/`, `ui/` on the `comms` branch.

### Words for the group people join ("server" / "space")

| Where | What it says now | Size |
|---|---|---|
| App text (`comms/app/ui/app.js`, `index.html`) | "Join a server", "Create a server", "Your servers", "Server settings", "Delete server", "Leave server", "Server created", `#servers`, `#add-server` | about 40 strings |
| App code | `space` field and `data-space` attribute everywhere (`app.js` ~42 lines, `web.rs` ~23, `client/src/lib.rs` ~42, `store.rs`) | ~120 |
| Server API (`server/src/api.rs`, 91 lines) | routes `/v1/spaces`, `/v1/spaces/{space}/channels|invites|members|bans|icon|channel-order|transfer`; `PATCH/DELETE /v1/spaces/{space}` | ~25 routes |
| Server code | `settings.rs` (67), `voice.rs` (20), `live.rs`, `blobs.rs`, `db.rs` | ~115 |
| Database | tables `spaces`, `members(space, ...)`, plus `space` columns on channels, invites, bans | existing test databases need a migration |
| Tests | `server/src/tests.rs` (44), `client/tests/e2e.rs`, `media/tests/call.rs` | ~65 |
| Settings | `COMMS_ADMIN_ONLY_SPACES`, `COMMS_SPACE_QUOTA_MB` (README, both compose files, `server/src/lib.rs`) | 2 env vars |
| Docs | `PROTOCOL.md` ("Spaces, channels and invites", "Server settings"), README | ~30 |
| Lodestone itself (`ui/`) | invite sending says "server"; `style.css` has ~26 `space`-matching lines (mostly unrelated CSS `space-between`, check before any search-and-replace) | small |

### Words for the machine ("host")

| Where | What it says now | Size |
|---|---|---|
| App text | "Set up a new host", "Your hosts", "Servers live on a host" | ~15 strings |
| App/client code | `host` as variable, type, file and module name: `client/src/host.rs`, `app/src/hosts.rs`, `Client.host`, `data-host` | ~330 lines |
| Server | `host_admin` column, "host admins", `COMMS_*` text saying "host" | ~20 |
| Docs | PROTOCOL.md (49 lines), README (10): "Run a host", "The host never sees message text" | ~60 |

Note the Rust crate is already `lodestone-comms-server`, so the crate and Docker names are already right
under the new vocabulary. Only "host" in the code and UI changes to "server".

### Words for the plug-in itself ("Comms")

| Kind | Current value | Where |
|---|---|---|
| Display name | "Lodestone Comms" | UI title and Home screen, `hello` message in `bridge.rs`, `plugins.rs`, invite page text in `server/src/api.rs`, share text "Join me on Lodestone Comms", error messages, GitHub release title, `network-hosts.txt` comments |
| Plug-in id | `comms` | `plugins.rs` (`COMMS`), `settings.rs` approved-permissions key, `plugin_sources.rs` (PR #7), `ui/app.js` (`PLUGIN_VIEW`, view name `comms`, `#comms` element, `comms-open` CSS class) |
| Flatpak app id | `io.github.daigonstar.Lodestone.Comms` | `plugins.rs`, `comms/flatpak/*.yml` and its file name, `build.sh`, `release-comms.sh`, `media/src/main.rs`, `PLUGIN.md`; also the data folder `~/.var/app/<id>/` |
| Programs | `lodestone-comms` (plug-in), `lodestone-comms-server`, `lodestone-comms-client`, `lodestone-comms-media`, `lodestone-comms-mediacheck`; Docker image/container `lodestone-comms` | Cargo manifests, Dockerfile, compose files, Flatpak manifest |
| Release tag prefix | `comms-v<version>` | `comms_install.rs`, `plugin_sources.rs` (PR #7), `release-comms.sh`, tests |
| Release file names | `lodestone-comms-<v>-linux-x86_64.flatpak`, `lodestone-comms-<v>-windows-x86_64.zip`, `SHA256SUMS` | `comms_install.rs`, `release-comms.sh` |
| Windows layout | `%LOCALAPPDATA%\Lodestone\Comms`, inner folder `Lodestone Comms\bin\lodestone-comms.exe`, test-build data in `%APPDATA%\Lodestone Comms` | `comms_install.rs`, `app/src/main.rs` |
| Local files | `<data>/plugins/comms-version.txt`, launch log `lodestone-comms` | `comms_install.rs`, `plugins.rs` |
| Tauri commands / events | `comms_status`, `comms_check_update`, `comms_install`, `comms_start`, `comms_approve`, `comms_theme`; events `comms-ready`, `comms-stopped`, `comms-open-chat` | `main.rs`, `plugins.rs`, `ui/app.js` |
| Environment variables | `COMMS_PUBLIC_URL`, `COMMS_BIND`, `COMMS_DATA`, `COMMS_DOMAIN`, `COMMS_PORT`, `COMMS_MAX_FILE_MB`, `COMMS_MEDIA_ADDR`, `COMMS_MEDIA_BIND`, plus the two `SPACE` ones above | server, README, compose files |
| Wire protocol strings | `lodestone-comms/v1` (prefix of every signed sign-in string), `/.well-known/lodestone-comms`, WebSocket subprotocol `lodestone-comms.v1`, MLS export label `lodestone-comms voice v1`, voice key info `lodestone-comms voice sender` | `server/src/auth.rs`, `api.rs`, `client/src/lib.rs`, `host.rs`, `discover.rs`, `media/src/frames.rs`, PROTOCOL.md |
| Bridge/API | bridge `hello.name` = "Lodestone Comms"; methods `friends`, `playing`, `open_chat`; permissions `friends:read`, `game:read`, `chat:open` | `bridge.rs`, `plugins.rs`, PLUGIN.md |
| Scripts/CI | `scripts/release-comms.sh`, `check-network.py` (globs `comms/`), `packaging/flatpak/cargo-sources.py` | |
| Folder | `comms/` in the repo | PR #6 targets it |

What does **not** need to change whatever name is picked: the bridge method and permission names
(`friends`, `playing`, `open_chat`) are generic, and "channel", "account", "device", "invite", "owner",
"admin", "member", "pairing", "setup code", "safety number" read fine in the new vocabulary.

## 2. Proposed vocabulary

| Concept | New word | Replaces | Note |
|---|---|---|---|
| The self-hosted program/machine | **Server** | host | One Docker container at one address. Matches the existing `lodestone-comms-server` crate. |
| The group people join | **Community** | server, space | What the protocol calls `community`; no more `space`. |
| A room inside a community | **Channel** | (unchanged) | Text now, voice later. |
| Person on a server | **Account** | (unchanged) | |
| Their install | **Device** | (unchanged) | |
| Admin of the whole machine | **Server admin** | host admin | The one who used the setup code. |
| Roles inside a community | **Owner, Admin, Member** | (unchanged) | Say "community admin" only where it could be confused with server admin. |
| The server's address | **Server address** | host address | |
| Joining | **Join a community** (with an invite) | Join a server | |
| Creating | **Create a community** | Create a server | |
| Adding a machine to your app | **Connect to a server** | Set up a new host | "Set up" is for creating one; connecting is what a user does. |
| Home screen list | **Your communities** / **Your servers** | Your servers / Your hosts | |
| Voice relay on the server | **Media relay** | voice relay | Optional wording fix. |

One sentence for the docs: *"A **server** is the program you run. It can hold many **communities**, each
with its own **channels** and members."*

Rule of thumb for users: you **join communities**, you **connect to servers**.

## 3. Candidate names for the plug-in

All keep the "Lodestone" prefix, so the Flatpak id stays `io.github.daigonstar.Lodestone.<Name>`.

| Name | Why it fits | Watch out for |
|---|---|---|
| **Lodestone Comms** (keep) | Already everywhere; says what it is; zero cost. | Plain, and a lodestone isn't about talking. "Comms" is also a generic term other projects use. |
| **Lodestone Beacon** | A beacon calls people to a place, the same family as a lodestone that guides. Short, easy to say: "open Beacon". The server could be "a beacon" in friendly text. | Common word, also a Minecraft block; check name collisions (Flathub, GitHub) before committing. |
| **Lodestone Camp** | Players gather at a camp; communities are "camps"? Warm, gaming feel. | "Camp" as the plug-in name may confuse with "community". Playful, less serious for a private/secure product. |
| **Lodestone Lodge** | Lodge fits the stone family; a lodge holds communities. | Sounds like a clubhouse, not a chat app; "Lodestone Lodge" is a mouthful. |
| **Lodestone Hail** | Hailing = calling someone; short; no collisions I know of. | Less obvious meaning; hail is also weather. |

## 4. What a rename touches and costs

The vocabulary change (server/community) and the plug-in name change are separate jobs. Do the vocabulary
change whatever name wins.

### A. Vocabulary: space to community, host to server

| Part | Work | Cost | Risk |
|---|---|---|---|
| UI text | Reword ~55 strings | small | low |
| Protocol and server | Routes `/v1/spaces` to `/v1/communities`, JSON field names, DB tables/columns, 2 env vars; DB migration (rename tables/columns) so test servers keep their data | medium | medium: a wire break between old and new apps/servers |
| Client/app code | `space` to `community`, `host` to `server` (file names `host.rs`, `hosts.rs`) | medium: mostly mechanical, ~450 lines | low; the compiler and the 60+ tests catch misses |
| Docs | PROTOCOL.md, README, PLUGIN.md, compose comments | small | low |

The wire break matters only for people already running a test server. The protocol is still marked
**draft**, there is no release yet (the Windows test build is `windows-test-5`), so now is the cheapest
time. Suggested approach: no `v2` needed; do it before the first release and tell testers to update
their server and app together. If a test server with real data has to survive,
the DB migration renames in place (reversible with a backup copy of the file).

### B. Plug-in name (only if not keeping "Comms")

| Part | Work | Cost | Risk |
|---|---|---|---|
| Display name | One constant plus ~40 strings, release title, `network-hosts.txt` comments | small | low |
| Plug-in id (`comms` to new id) | `plugins.rs`, `plugin_sources.rs`, `ui/app.js`, settings key. Approved permissions are stored under the old id: either migrate the key or let testers re-approve once | small | low |
| Flatpak app id | **Changing it makes it a different app**: users lose `~/.var/app/<id>/` data (their keys and chat history), unless the app copies/migrates it. Keep the same id and name only the display text if testers have data they care about. | medium | **high for existing testers** |
| Release tag prefix `comms-v` | Change in `comms_install.rs`, `plugin_sources.rs`, `release-comms.sh`, tests. Existing `comms-v*` releases (drafts/prereleases) stay on GitHub; the installer would stop seeing them. Re-publish under the new prefix, or have the installer accept both for one version | small | low |
| Release file names, Windows folders | `lodestone-comms-<v>-...` to the new name; Windows install folder moves | small | testers reinstall once |
| Crate and binary names | `lodestone-comms*` to the new name; Cargo.lock and `flatpak/cargo-sources.json` regenerate | medium | low |
| Protocol strings (`lodestone-comms/v1`, `.well-known`, WS subprotocol, MLS labels) | These are part of every signature and key derivation. Renaming is only safe **before** anything real ships; afterwards it breaks every server and app. | small now, **very costly later** | high after release |
| Repo folder `comms/` | `git mv`; PR #6 (`comms` into `main`) and the `check-network.py` globs follow | small | low |

### PR #7 (plug-in installer)

PR #7 hard-codes the old names in `plugin_sources.rs`:
`Source { id: "comms", repo: "daigonstar/lodestone", tag_prefix: "comms-v", official: true }`, with tests
that use `comms-v0.x.0`, and `comms_install.rs` carries the Flatpak id, file names and Windows folders.
So:

1. **Land the naming decision before or together with PR #7**, otherwise it gets renamed twice.
2. If only the vocabulary changes (server/community), PR #7 is unaffected: it never mentions spaces.
3. If the plug-in is renamed, PR #7 needs the new id, tag prefix and file names in one place. Better: move
   the id, display name, Flatpak id, tag prefix and file-name pattern into a single `Source` record so a
   later rename touches one struct, not five files.
4. The installer must keep accepting only official sources and checksum-verified files (AGENTS.md rules 5
   and 8); a rename must not loosen that.

### AGENTS.md rules

Renaming touches `network-hosts.txt` comments and README wording (rule 2, rule 8). Nothing here adds a
network address, so no new hosts. The release/installer files (`comms_install.rs`, `plugin_sources.rs`)
are in the "extra care" list; the commit must call that out.

## 5. Recommendation

1. **Do the vocabulary change now** (server = machine, community = group, `space` gone). It's the part
   that is confusing users and docs today, and it's cheapest before the first real release.
2. **Pick a plug-in name now, before PR #7 merges and before any public release**, because protocol
   strings, the Flatpak id and the tag prefix get expensive to change afterwards.
3. My pick: **Lodestone Beacon**, with **Lodestone Comms** as the safe fallback. Reason: "Comms" is
   descriptive but generic and doesn't feel like part of the Lodestone family; Beacon is short, fits the
   guiding-stone theme, and you can say "join a community on a Beacon server" naturally. If you'd rather
   spend no effort, keep Comms and only do step 1.
4. If a rename is chosen, do it in this order, one PR each, all into `comms`:
   1. vocabulary (server/community) across UI, protocol, code, docs, with a DB migration;
   2. plug-in name: display text, id, tag prefix, file names, protocol strings, crates, Flatpak id
      (testers reinstall once; their keys and history are lost unless we add a copy step, tell me if
      anyone has chats worth keeping);
   3. adjust PR #7 to the final names (or merge PR #7 first and rename after, accepting a second pass).

## 6. Questions for Robb

1. Do you confirm **server** (machine) / **community** (group) / **channel** as in section 2?
2. Which plug-in name: Comms (keep), Beacon, Camp, Lodge, Hail, or one of your own?
3. Does anyone have a test server or chat history worth migrating, or can testers start clean?
4. Should the rename land before PR #7 (my suggestion), or after?
