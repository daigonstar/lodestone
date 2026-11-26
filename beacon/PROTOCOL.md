# Lodestone Beacon protocol (draft, version 1)

Lodestone Beacon is text chat, voice calls, cameras and screen sharing in communities that run on servers people host themselves.
This document is the contract between the Beacon server program and its clients. It's public so anyone can
check exactly what a server sees and stores. Status: **draft**, it will change before the first release.

## Words used here

- **Server:** one running Beacon server program (one Docker or Podman container), reachable at its public URL.
  Not to be confused with a Discord "server": here that group is a community.
- **Community:** the group people join, with its own channels and members. A server can hold many communities.
- **Channel:** a text channel in a community (voice channels come later).
- **Account:** a username on a server. An account has one or more **devices**.
- **Device:** one install of the client. It has an Ed25519 signing key, made on the device; the private key
  never leaves it.

## What a server can and can't see

Messages are end-to-end encrypted with MLS (Messaging Layer Security, RFC 9420). The server relays and
stores ciphertext it can't read.

| The server sees | The server never sees |
|---|---|
| usernames and device public keys | message text, attachments, reactions |
| community and channel names, and who is a member | any private key |
| when messages are sent, their size, and by which device | passwords (there are none: devices sign in with their key) |
| the size of attached files and when they were sent | what's in attached files, their names or types |
| your chosen status and status message, and when your app last checked in | |
| who is in which call, mute and deafen state, and the size and timing of encrypted audio | what anyone says in a call |
| who has their camera on and who is seeing cameras, who is sharing their screen and who is watching, the size and timing of encrypted video, and each frame's readable VP8 header (keyframe or not, and the picture's width and height) | what's on anyone's screen or camera |

The server's logs never contain session tokens or invite codes.

## Finding a server by its domain

`GET /.well-known/lodestone-beacon` returns `{ "protocol": "lodestone-beacon/v1", "public_url": "...", "version": "..." }`.
A client given only a domain (`beacon.example.com`) asks `https://<domain>/.well-known/lodestone-beacon`,
checks the protocol, and uses `public_url` as the server's address, provided it's HTTPS on that same domain.

Every answer from a server carries `Cache-Control: no-store`, so proxies and CDNs never cache it.

## Signing in: device keys, no passwords

All signed strings start with `lodestone-beacon/v1`, and include the server's public URL so a signature for
one server can't be replayed on another.

1. **Register** (`POST /v1/register`): `{ username, device_key, signature, invite }`
   - `device_key`: the device's Ed25519 public key, base64 (standard, padded).
   - `signature`: signs `lodestone-beacon/v1 register <server url> <username> <device_key>`.
   - `invite`: an invite code (the account joins that community) or the server's **setup code**. A server doesn't
     allow registration without one. The setup code is printed once in the server's log on first start; the
     account that uses it becomes the server's admin.
   - Usernames: 2 to 32 characters, `a-z 0-9 _ . -`, case-insensitive, unique on the server.
2. **Add a device by pairing**, approved on a device you already have:
   - The existing device: `POST /v1/pairing` returns a one-time `code` (10 minutes; at most 3 waiting).
   - The new device: `POST /v1/pairing/claim` `{ code, device_key, signature }`, signature over
     `lodestone-beacon/v1 pair <server url> <code> <device_key>`. Returns the username and the new device's
     **fingerprint** (the first 8 bytes of SHA-256 of its key, like `8db0-cc19-ae79-87d1`). A code can only
     be claimed by one device. It then polls `POST /v1/pairing/status` `{ code, device_key }`
     (`waiting`, `approved`, `refused`).
   - The existing device: `GET /v1/pairing` lists waiting devices with their fingerprints; the person
     compares the fingerprint on both screens, then `POST /v1/pairing/approve` or `/refuse` `{ device_key }`.
     Only an approved device is added, so a leaked code alone isn't enough.
   - Members' apps then add the new device to every channel. It can read messages from then on, not
     older ones.
   - `POST /v1/devices/remove` `{ device_key }` removes one of your other devices; members' apps take it
     out of every channel. (`POST /v1/devices` adds a device directly when signed in on the account; the
     apps use pairing.)
3. **Sign in** on a device:
   - `POST /v1/challenge` `{ device_key }` returns `{ nonce }` (valid for 2 minutes, single use).
   - `POST /v1/session` `{ device_key, nonce, signature }`, signature over
     `lodestone-beacon/v1 login <server url> <nonce>`, returns `{ token, expires }`.
   - The token is 32 random bytes (base64url). The server stores only its SHA-256. Send it as
     `Authorization: Bearer <token>`. Sessions last 30 days.

## Identity: one key that belongs to the person

Devices sign in with their own keys (above). On top of that, a person has an **identity**: one more Ed25519
key made by their app on first run. It is never sent anywhere; servers only ever see its public half. It is
backed up as a 24-word recovery phrase (the key's 32 bytes as BIP-39 words); whoever has the phrase is that
person. The point: the same person is recognisable across servers, and can get back into their accounts on a
new device. Nobody holds an account database: each server stores only public keys.

- **Registering with an identity.** `POST /v1/register` also takes `identity_key` (base64 public key) and
  `identity_signature` over `lodestone-beacon/v1 identity <server url> <username> <device_key>`. The server
  stores the key on the account (`GET /v1/me` and community member lists show it as `identity`). One account
  per identity per server (a second one gets 409); usernames stay unique per server, and a person may use a
  different one on each server. Clients show the identity's fingerprint (SHA-256 of the base64 key, first 8
  bytes, as `3f9a-02c1-77de-b410`).
- **Recovering on a new device** (no approval from another device, because the identity key proves who you are):
  - `POST /v1/recover/challenge` `{ identity_key }` returns `{ nonce }` (2 minutes, single use). The answer is
    the same whether or not the identity has an account here.
  - `POST /v1/recover` `{ identity_key, device_key, nonce, identity_signature, device_signature }`.
    `identity_signature` is over `lodestone-beacon/v1 recover <server url> <identity_key> <device_key> <nonce>`
    and `device_signature` over `lodestone-beacon/v1 recover-device <server url> <device_key> <nonce>`, so the
    identity vouches for this exact device and this device holds its own key. The device is added to the
    identity's account (at most 10 devices), and members' apps add it to the channels as for any new device. It
    reads messages from then on, not older ones.
- **Pairing can hand over the identity.** `POST /v1/pairing/approve` takes an optional `blob` (up to 1 KiB):
  the identity, AES-256-GCM encrypted by the approving app with a key that only travels in the pairing QR code
  (`beacon-pair:1|<server url>|<code>|<key>`). `POST /v1/pairing/status` returns it once with `approved`. The
  server never sees the key, so it can't read the blob; the status call has no sign-in, so anyone who knows the
  new device's key could take the (still encrypted) blob first, which would make that device fall back to
  asking for a recovery phrase.
- **Adding a PC by scanning its QR code with your phone** (the other direction, for a new device with no camera
  or no account yet). The new PC shows a QR code `beacon-pair:2|<server url>|<device_key>|<ts>|<signature>|<key>`:
  its device key, the time it made the code, its own signature over `lodestone-beacon/v1 adopt <server url> <ts>
  <device_key>` (proof that the key belongs to the device showing the code) and a random key for sealing the
  identity. The phone, signed in on that server, shows the device's fingerprint; the person compares it with the
  PC's screen and approves with `POST /v1/pairing/adopt` `{ device_key, ts, signature, blob? }`. The server checks
  the signature (bound to this server), that the code is at most 10 minutes old, that the device isn't registered
  and that the account has fewer than 10 devices, then adds the device and keeps `blob` (up to 1 KiB, the identity
  sealed with the key from the QR code) for the PC to collect once. The PC polls `POST /v1/pairing/status` with
  its `device_key` (and any `code`): when it is `approved` the answer also carries the `username`, so a PC that has
  never had an account learns it. Anyone who can see the QR code can make their own phone adopt that PC, so the
  PC's fingerprint must be compared and the code is short-lived.
- **Not done yet:** clients don't pin or warn when a username shows up with a different identity, and old
  messages don't travel to a new device (see Project.md).

## Communities, channels and invites

| Request | Who | Does |
|---|---|---|
| `GET /v1/me` | signed in | your account, devices and communities |
| `POST /v1/communities` `{ name }` | signed in (or only server admins, if the server is set up that way) | creates a community you own, with a `general` channel |
| `GET /v1/communities/{community}` | member | the community, its channels and members |
| `POST /v1/communities/{community}/channels` `{ name }` | owner or admin | adds a text channel |
| `POST /v1/communities/{community}/invites` `{ max_uses, expires_in }` | member | an invite code (`max_uses` 1 to 100, `expires_in` up to 30 days, in seconds) |
| `GET /v1/invites/{code}` | anyone with the code | the community's name and member count, to show before joining |
| `POST /v1/invites/{code}/accept` | signed in | joins the community |
| `DELETE /v1/communities/{community}/members/{username}` | owner or admin (or yourself, to leave) | removes the member; `?ban=true` also stops them rejoining |

Roles: **owner** (one per community), **admin**, **member**. Names: 1 to 64 characters, no control characters.

### Community settings

| Request | Who | Does |
|---|---|---|
| `PATCH /v1/communities/{community}` `{ name?, description? }` | owner or admin | renames the community or sets its description (up to 500 characters) |
| `PUT /v1/communities/{community}/icon` (image body) / `DELETE` | owner or admin | sets or removes the icon: PNG, JPEG, GIF or WebP, up to 256 KiB, checked by its first bytes |
| `GET /v1/communities/{community}/icon` | member | the icon, with `nosniff` and a CSP that runs nothing |
| `PUT /v1/communities/{community}/banner` (image body) / `DELETE` | owner or admin | sets or removes the banner shown on the community's home-page card: same image rules as the icon, up to 512 KiB |
| `GET /v1/communities/{community}/banner` | member | the banner, served like the icon |
| `POST /v1/communities/{community}/categories` `{ name }` | owner or admin | adds a channel category (up to 50) |
| `PATCH /v1/categories/{category}` `{ name }` / `DELETE` | owner or admin | renames a category; deleting one keeps its channels, uncategorised |
| `PUT /v1/communities/{community}/category-order` `{ categories }` | owner or admin | sets the category order (every category once) |
| `DELETE /v1/communities/{community}` | owner | deletes the community with its channels, messages and invites |
| `PATCH /v1/channels/{channel}` `{ name?, topic?, category?, parent?, post_rule? }` / `DELETE` | owner or admin | renames a channel, sets its topic (up to 300 characters), moves it into a category (`null` for none), makes it a child of a top-level text channel (`parent`, one level only; `null` for top level), or sets who can post (`post_rule`: `everyone` or `admins`; the server rejects `application` messages from other members, commits and proposals still pass). Deleting a channel (a community keeps at least one) moves its children to the top level |
| `PUT /v1/communities/{community}/channel-order` `{ channels: [...] }` | owner or admin | the channel order; must list every channel once |
| `PUT /v1/communities/{community}/members/{username}/role` `{ role }` | owner | `admin` or `member` |
| `POST /v1/communities/{community}/transfer` `{ username }` | owner | makes another member the owner; the old owner becomes an admin |
| `GET /v1/communities/{community}/bans`, `DELETE /v1/communities/{community}/bans/{username}` | owner or admin | lists and lifts bans |
| `GET /v1/communities/{community}/invites`, `DELETE /v1/communities/{community}/invites/{id}` | member (own invites) or admin (all) | active invites by a short ID (the start of the code's hash: codes can't be shown again), and revoking them |

Admins can remove and ban members, but not other admins or the owner. Descriptions, icons and topics are
community metadata the server sees, like names; messages stay end-to-end encrypted.

Invite links have the form `<server url>/join/<code>`. Lodestone can send one to a friend from its friends
list: it opens that friend's Steam chat with the link ready to paste (Steam offers no way for other apps
to send messages), or copies it for Epic friends.

## End-to-end encryption (MLS)

- Each text channel is one MLS group; the channel ID is the MLS group ID. Ciphersuite:
  `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519`.
- Each device's MLS credential is a basic credential whose identity is `<username>:<device_key>` (a colon can't appear in either), signed
  with the device key. Clients check that the username and key match what the server lists for that member,
  and show a **safety number** that members can compare to make sure the server hasn't swapped a key.
- The server is MLS's **delivery service**: it doesn't parse MLS messages, it stores and forwards them in
  order.

### Key packages
- `POST /v1/keypackages` `{ key_packages: [base64, ...] }`: a device uploads a batch (up to 100 waiting).
- `GET /v1/keypackages`: how many of your device's key packages are still waiting.
- `POST /v1/keypackages/claim` `{ username }`: returns one key package per device of that user and
  removes them from the server (single use).

### Sending into a channel
`POST /v1/channels/{channel}/messages` `{ kind, epoch, body }`, members only.
- `kind`: `commit`, `proposal` or `application` (an encrypted chat message).
- `epoch`: the MLS epoch the sender is in.
- `body`: the MLS message, base64, at most 64 KiB.
- The server gives each accepted message a sequence number and sends it to members in that order.
- **Commits are ordered by the server:** a commit is only accepted if its `epoch` equals the channel's
  current epoch, which then goes up by one. Otherwise the answer is `409` and the client catches up and
  tries again. This keeps everyone on one history when two members commit at once.

`GET /v1/channels/{channel}/messages?after=<seq>&limit=<n>` returns stored messages after a sequence
number (`limit` up to 500). New members can only read messages from the epoch they were added in: older
messages were encrypted before they had keys. This is by design.

### Welcomes
`POST /v1/welcomes` `{ channel, to: [device_key, ...], body }`: the member who adds someone sends the MLS
Welcome to the new devices' mailboxes. Devices collect them over the live connection or with
`GET /v1/welcomes` (removed once delivered).

### Joining and leaving
- When someone joins a community, the server tells online members (`member_joined`). A member's client claims
  the newcomer's key packages, commits an Add to each channel group and sends the Welcome. If two clients
  try at once, the epoch rule lets one win; the other sees the member is already in.
- When someone leaves or is removed, the server stops sending them anything at once, and a member's client
  commits a Remove so their keys no longer work for new messages.

## Attachments

A file is encrypted on the sender's device with AES-256-GCM, using a new random 256-bit key and 96-bit
nonce for each file. The ciphertext goes to the server; the key travels only inside the end-to-end
encrypted message, so the server can't read the file.

- `POST /v1/channels/{channel}/blobs` (members), the ciphertext as the body: returns `{ id, size }`. At
  most `BEACON_MAX_FILE_MB` (default 10) per file and `BEACON_COMMUNITY_QUOTA_MB` (default 2048) per community;
  30 uploads a minute per device.
- `GET /v1/blobs/{id}` (members of the file's community): the ciphertext, served as an inert download.
- Files are deleted with their channel or community.

The message that carries a file is an application message with `t: "file"`, the caption as `body`, and
`file: { id, name, mime, size, key, nonce }`. Receivers decide whether it's an image from the decrypted
content (PNG, JPEG, GIF, WebP), never from the sender's claim, and a file changed on the server fails to
decrypt and isn't shown. The server sees each file's size and when it was uploaded.

## Profiles

A profile (display name up to 32 characters, a name colour `#rrggbb`, a picture up to 256 KiB) is an
application message with `t: "profile"`, the display name as `body`, `color`, and the picture as an
encrypted attachment in `file`. The server never sees names or pictures. A client sends its profile to
every channel when it changes, and again into a channel whenever members are added to it (newcomers
can't read anything older). Receivers keep only a verified sender's own profile, so nobody can set
someone else's; clients show the real `@username` beside a display name.

What someone is playing is an application message with `t: "activity"` and the game's title (up to 100
characters, one line) as `body`; an empty body means nothing. Clients send it to every channel when it
changes and every 5 minutes while playing; receivers keep it in memory only and forget it after 12
minutes without a repeat, so a client that vanished doesn't leave someone "playing" forever. It isn't
sent while the sender's status is invisible. The server never sees it.

Mentions (`@username` in a message) are found by each client after decrypting: the server never learns
who was mentioned.

## Status

- `PUT /v1/me/status` `{ status, text }`: `online`, `away`, `busy` or `invisible`, and an optional message
  (up to 80 characters).
- Each member in `GET /v1/communities/{community}` has `presence` (`online`, `away`, `busy` or `offline`) and
  `status_text`. Someone whose app hasn't checked in for 45 seconds, or who is invisible, shows as
  `offline`, with no message.
- The server keeps "last checked in" in memory only. Statuses and messages are visible to the server.

## Calls (voice channels)

Channels have a `kind`: `text` (default) or `voice` (`POST /v1/communities/{community}/channels` `{ name, kind }`).
Each voice channel is also an MLS group, like a text channel; its members' devices are added the same way.

The server runs a media relay (an SFU) on one UDP port. It forwards each participant's audio to the others
without decoding it. Clients encrypt every audio frame before sending, so the relay can't hear calls:

- `base`: 32 bytes from the voice channel's MLS group for the current epoch (MLS exporter, label
  `lodestone-beacon voice v1`, empty context). Every member derives the same base; the server can't.
- Each sender's key: HKDF-SHA256 over `base` with info `lodestone-beacon voice sender` followed by the
  sender's MLS leaf index (4 bytes, big-endian).
- Each frame: a 17-byte header `[version = 1][epoch: u32][leaf: u32][counter: u64]` (big-endian, used as
  AES-GCM associated data), then AES-256-GCM of the Opus frame with nonce = 4 zero bytes + counter.
  The leaf in the header picks the sender's key, so the relay's slot numbers can't make one person
  sound like another: a mismatched key just fails to decrypt.
- Keys change with every epoch (anyone joining or leaving the channel's group). Receivers keep a few
  older epochs' bases for frames sent just before a change.

- `GET /v1/channels/{channel}/voice` (members): `{ enabled, relay, relays, participants: [{ username, device, slot, muted, deafened }] }`
  (`relays` are the relay's addresses as `ip:port`, in the server's order, usually public then LAN;
  `relay` is the first). Clients try the one that fits their network first and fall back to the others.
  Channel lists in `GET /v1/communities/{community}` show `in_call` (usernames) for voice channels.
- `POST /v1/channels/{channel}/voice/join` `{ offer, relay, muted, deafened }` (`relay`: which of `relays`
  this device connects to; the first if left out): a WebRTC offer (JSON) with one
  send-only audio track and up to 16 receive-only ones, in that order. Returns `{ answer, slot, participants }`.
  The relay writes each participant's audio into the receive track matching their `slot` on everyone
  else's connection, so nobody needs to renegotiate when people join or leave. At most 16 people per call.
- `POST /v1/channels/{channel}/voice/state` `{ muted, deafened }`: deafened participants aren't sent audio.
- `POST /v1/channels/{channel}/voice/share` `{ on }`: you're sharing your screen (participants show `sharing`).
- `POST /v1/channels/{channel}/voice/watch` `{ slot, on }`: start or stop receiving the share of the
  participant in `slot`. Screen shares are only forwarded to people watching them, so a share costs the
  server upload per viewer, not per person in the call.
- `POST /v1/channels/{channel}/voice/camera` `{ on }`: your camera is on (participants show `camera`).
- `POST /v1/channels/{channel}/voice/cameras` `{ on }`: start or stop receiving everyone's cameras.
  Clients turn this on while their call view is open, so cameras cost the server upload only for people
  looking at them.
- Screen shares: the join offer also has one send-only video track and 16 receive-only ones (after the
  audio tracks), VP8, slots matching the audio ones. Cameras: another send-only video track and 16
  receive-only ones after those (1280x720, 30 frames a second, about 1.5 Mbps). A screen share's sound
  comes last: one send-only audio track and 16 receive-only ones (stereo Opus, 128 kbps), encrypted like
  voice, and forwarded only to that share's watchers. When someone starts watching, or a viewer reports
  a lost frame, the relay asks the sharer for a keyframe (RTCP PLI).
- Video frames are encrypted with the same sender keys, but laid out differently so the transport can
  still tell keyframes apart: the first bytes of the VP8 frame stay readable (10 on a keyframe, 3
  otherwise: frame type, version and picture size, nothing of the picture), then AES-256-GCM of the
  rest, then the 17-byte header (version 2) at the end. The readable bytes are authenticated with the
  header. Audio and video from one device share one counter, so nonces never repeat.
- `POST /v1/channels/{channel}/voice/leave`. Removing someone from a community also ends their calls in it.
- The relay is ICE-lite, advertising the address each client chose (from `BEACON_MEDIA_ADDR`); transport is DTLS-SRTP. With
  direct UDP, people in a call see that address (a home IP when self-hosted at home).

## Live connection

`GET /v1/live` upgrades to a WebSocket. The session token goes in the `Sec-WebSocket-Protocol` header as
`lodestone-beacon.v1, <token>` (not in the URL, so it can't end up in logs). The server sends JSON events:

- `message` `{ channel, seq, kind, epoch, sender, sender_name, body, sent_at }` (`sender` is the device key)
- `welcome` `{ channel }` (collect it with `GET /v1/welcomes`)
- `member_joined` / `member_left` `{ community, username }`
- `channel_created` `{ community, channel, name }`

## Limits

- A device may have at most 5 sign-in challenges waiting; 60 messages a minute per device.
- 64 KiB per message body. Attachments come later, as encrypted uploads.

## Later (not in version 1 yet)

- Sound from shared windows; choosing which camera with a preview.
- Message edits and deletes (as encrypted application messages).
- Safety-number checks between members (comparing everyone's keys out of band).
