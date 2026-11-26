# Lodestone plug-ins (draft, version 1)

This is the reference for the bridge. For a guide to building a plug-in, with a working example, see
[docs/PLUGINS.md](../docs/PLUGINS.md).

A plug-in is a separate program, normally its own Flatpak with its own sandbox, that shows its interface
inside Lodestone's window. Lodestone Beacon is the first. The design keeps the polish of one app and the
safety of two:

- The plug-in can't read Lodestone's files (store sign-ins, keys, cookies). It has its own data folder.
- Its interface is a page the plug-in serves on `127.0.0.1`, shown in a frame inside Lodestone. The
  frame can't use Lodestone's commands.
- Everything the plug-in can ask Lodestone for goes through the **bridge** below, method by method, and
  only for permissions you approved.

## Starting

Lodestone starts the plug-in (with the argument `--lodestone-bridge`) when you first open it, with its
standard input and output connected to Lodestone (nothing else can reach this channel). Lodestone looks
for the plug-in's Flatpak (`io.github.daigonstar.Lodestone.Beacon`) first, then a `lodestone-beacon`
program on the PATH. Messages are JSON, one per line.

The plug-in's first line is a `hello`:

```json
{"type":"hello","name":"Lodestone Beacon","version":"0.1.0","ui":"http://127.0.0.1:41234/#t=<secret>","permissions":["friends:read","chat:open","game:read"]}
```

- `ui`: the page to show. The part after `#` is a per-launch secret the page uses to call its own
  plug-in; it's never sent over the network.
- `permissions`: what the plug-in will ask for. The first time (and whenever the list grows) Lodestone
  shows them in plain words and asks you. Until you approve, Lodestone doesn't show the plug-in and
  answers every request with an error.

## Requests (plug-in to Lodestone)

```json
{"type":"request","id":7,"method":"friends","params":{}}
{"type":"response","id":7,"ok":[ ... ]}
{"type":"response","id":8,"error":"not allowed"}
```

| Method | Permission | What it gives |
|---|---|---|
| `friends` | `friends:read` | Your Steam and Epic friends: store, ID, name, avatar URL, online status. Nothing else about them. |
| `playing` | `game:read` | The title of the game you launched from Lodestone, while it's running (or null). Beacon shows it to people in your communities, end-to-end encrypted. Nothing about other programs. |
| `open_chat` `{store, id, text}` | `chat:open` | Puts `text` on your clipboard and opens that friend's Steam chat, so you can paste it. Lodestone shows a note saying so. It never sends anything itself. |

The bridge never offers sign-in data, keys, cookies, file paths, or other running programs (with
`game:read`, only the title of the game you launched from Lodestone).

## Events (Lodestone to plug-in)

```json
{"type":"event","name":"theme","data":{ "--bg": "#0f1117", "...": "..." }}
```

- `theme`: Lodestone's colours, so the plug-in looks native.

## Stopping

Lodestone closes the plug-in's standard input when it quits; the plug-in then exits.
