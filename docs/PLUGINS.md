# Making a Lodestone plug-in

This is for people who want to build something that lives inside Lodestone, the way Lodestone Beacon
does. It is written from how Beacon and Lodestone work today, and it says plainly what isn't built yet.

## What a plug-in is, and what it isn't

A plug-in is a **separate program** that shows its interface inside Lodestone's window. It is not code
that runs inside Lodestone, and it never gets Lodestone's secrets:

- It can't read Lodestone's files (store sign-ins, tokens, keys, cookies). Normally it is its own
  Flatpak with its own sandbox and its own data folder.
- Its interface is a **web page the plug-in serves on 127.0.0.1**, shown in a frame inside Lodestone.
  The frame can't use Lodestone's commands.
- Everything it can ask Lodestone for goes through the **bridge**: JSON lines over the plug-in's
  standard input and output, one request at a time, and only for permissions the user approved.

If you can write a program that serves a web page and reads and writes JSON lines, in any language, you
can write a plug-in. The reference for the exact messages is [beacon/PLUGIN.md](../beacon/PLUGIN.md); this
guide explains how to put one together.

## The contract in five steps

1. **Start.** Lodestone starts your program with the argument `--lodestone-bridge`, with standard input
   and output connected to it. Without that argument there is no bridge (so you can run your plug-in on
   its own while developing). Never print anything else on standard output: it is the bridge.
2. **Serve your page** on `127.0.0.1`, on any free port. Make up a random secret for this launch.
3. **Say hello.** Your first line is
   `{"type":"hello","name":"…","version":"…","ui":"http://127.0.0.1:PORT/#t=SECRET","permissions":[…]}`.
   `ui` is the page to show; the part after `#` is the secret your page uses to call your own program
   (it stays in the browser, it's never sent over the network). `permissions` lists what you will ask for.
   Until the user approves them, Lodestone doesn't show your page and answers every request with an error.
4. **Ask for things** with `{"type":"request","id":1,"method":"playing","params":{}}`. Lodestone answers
   `{"type":"response","id":1,"ok":…}` or `{"type":"response","id":1,"error":"…"}`. Be ready for errors:
   the user may not have approved that permission.
5. **Quit when your input closes.** Lodestone closes your standard input when it quits or when the
   user stops the plug-in. Exit then (within two seconds, or you are killed).

Lodestone also sends `{"type":"event","name":"theme","data":{"--bg":"#…","--text":"#…",…}}` with its
colours (`--bg`, `--bg-2`, `--bg-3`, `--line`, `--text`, `--muted`, `--accent`, `--danger`) so your page
can look native. Lodestone sends it each time the user opens the plug-in's view, so apply the latest one you got and don't assume it arrives only once.

### What you can ask for today

| Method | Permission | What it gives |
|---|---|---|
| `friends` | `friends:read` | The user's Steam and Epic friends: store, ID, name, avatar URL, online status. Nothing else. |
| `playing` | `game:read` | The title of the game the user launched from Lodestone, while it runs (or `null`). |
| `open_chat` `{store, id, text}` | `chat:open` | Copies `text` and opens that friend's Steam chat, so the user can paste. It never sends anything. |

Lodestone never offers sign-in data, keys, cookies, file paths or the ability to run programs, and it
won't grow a method that does. New methods are added only with a plain-words description the user sees
when asked to approve them.

## A working example

[docs/examples/hello-plugin](examples/hello-plugin) is a complete plug-in in one Python file (standard
library only): it serves a page, says hello, asks for `playing`, applies the theme and quits when its input
closes. Next to it, `test_hello_plugin.py` plays Lodestone's part and checks the contract:

```
python3 docs/examples/hello-plugin/test_hello_plugin.py
```

You can point that test at your own plug-in's program (`test_hello_plugin.py ./my-plugin`) to check
yours. It checks the hello, that your page is only framable by Lodestone, that your API refuses requests
without the token, the request/response round trip, the theme event and the clean exit. It does not
replace trying your plug-in in Lodestone.

## Keeping your page safe

Your page runs next to a user's other programs, so treat `127.0.0.1` as a place anyone on the machine, and
any web page the user opens, could try to reach. Lodestone Beacon does all of this, and the example shows
how:

- **Bind to 127.0.0.1 only**, never `0.0.0.0`.
- **Require the per-launch secret** on everything except the page's own files. Beacon sends it in a
  header (`x-beacon-token`, `x-plugin-token` in the example), never in a URL you log.
- **Check the `Host` header** is exactly your address and port, so a web page can't reach you through a
  name that points at 127.0.0.1.
- **Send a strict Content-Security-Policy**: `default-src 'none'`, scripts, styles and connections only
  from `'self'`, and `frame-ancestors tauri://localhost http://tauri.localhost` so only Lodestone's
  window can frame you. Don't use inline scripts or styles; that is what keeps a bug from running
  someone's HTML.
- **Never put secrets in the page** or in anything shown to the user, and never log tokens.
- **Don't trust what you receive.** Names, titles and messages from other people are text, not HTML:
  insert them with `textContent`.

## Packaging

- **Flatpak** is the normal way. Give it an application ID of the form `io.github.<you>.Lodestone.<Name>`
  and ask for as little as you can. Beacon's manifest
  ([beacon/flatpak](../beacon/flatpak/io.github.daigonstar.Lodestone.Beacon.yml)) asks only for the network and
  sound, and gets screens and cameras through the desktop's portals, which ask the user each time. Your
  data lives in `~/.var/app/<id>/`.
- Lodestone looks for the plug-in's Flatpak first, then for a program on the `PATH` (handy while
  developing). On Windows, Lodestone installs a zip into its own folder.
- If your plug-in talks to a service of its own, say so plainly in your README and keep a list of every
  address it uses. Lodestone's own rules (see [AGENTS.md](../AGENTS.md)) are a good baseline for you:
  no telemetry, no hidden addresses, nothing sent anywhere the user's own action doesn't need.

## Releasing so Lodestone can install it

This part is how Beacon works today; see "What isn't built yet" for what a second plug-in would need.

- Publish releases on GitHub with a tag of the form `<name>-v<version>` (Beacon uses `beacon-v0.2.0`).
- Attach one file per platform with a predictable name, for example
  `lodestone-beacon-<version>-linux-x86_64.flatpak` and `lodestone-beacon-<version>-windows-x86_64.zip`.
- GitHub publishes a SHA-256 checksum (the asset's `digest`) for each file. **Lodestone refuses to install
  a file without one**, and checks every download against it before anything is installed. Installing
  and updating only happens when the user presses the button.

## What isn't built yet

Be aware of these before you start; they are why Beacon is "the only plug-in so far":

- **Lodestone only knows Beacon.** The plug-in id, its Flatpak ID, its release tag and its list of
  permissions are written into Lodestone (`src-tauri/src/plugins.rs` and `beacon_install.rs`). A second
  plug-in can run with the same bridge, but Lodestone can't list, install or approve it until that code is
  made general. This is planned, with an official list of plug-ins built into Lodestone (reviewed, with
  checksums) and, separately, a way for a user to add a third-party source from their own settings. A
  clean install will have no third-party sources, and AGENTS.md rules 2 and 5 stay as written.
- **The bridge is version 1 and a draft.** Methods and messages may change before 0.3.0.
- **Nothing here has been tested on Windows**, and the example has only been run against the test that
  plays Lodestone's part, not inside Lodestone itself.

If you are thinking of building a plug-in, say so in an issue first: the part Lodestone has to do is small
but it is Lodestone's to change, and it's better to settle the permissions you need before you build.
