# Lodestone Comms protocol (draft, version 1)

Lodestone Comms is text chat (and later voice, video and screen share) on servers people host themselves.
This document is the contract between the Comms host program and its clients. It's public so anyone can
check exactly what a host sees and stores. Status: **draft**, it will change before the first release.

## Words used here

- **Host:** one running Comms server program (one Docker or Podman container), reachable at its public URL.
- **Server:** a community on a host, like a Discord server. A host can hold many. The protocol calls it a
  **space** so it isn't confused with the host.
- **Channel:** a text channel in a space (voice channels come later).
- **Account:** a username on a host. An account has one or more **devices**.
- **Device:** one install of the client. It has an Ed25519 signing key, made on the device; the private key
  never leaves it.

## What a host can and can't see

Messages are end-to-end encrypted with MLS (Messaging Layer Security, RFC 9420). The host relays and
stores ciphertext it can't read.

| The host sees | The host never sees |
|---|---|
| usernames and device public keys | message text, attachments, reactions |
| space and channel names, and who is a member | any private key |
| when messages are sent, their size, and by which device | passwords (there are none: devices sign in with their key) |

The host's logs never contain session tokens or invite codes.

## Signing in: device keys, no passwords

All signed strings start with `lodestone-comms/v1`, and include the host's public URL so a signature for
one host can't be replayed on another.

1. **Register** (`POST /v1/register`): `{ username, device_key, signature, invite }`
   - `device_key`: the device's Ed25519 public key, base64 (standard, padded).
   - `signature`: signs `lodestone-comms/v1 register <host url> <username> <device_key>`.
   - `invite`: an invite code (the account joins that space) or the host's **setup code**. A host doesn't
     allow registration without one. The setup code is printed once in the host's log on first start; the
     account that uses it becomes the host's admin.
   - Usernames: 2 to 32 characters, `a-z 0-9 _ . -`, case-insensitive, unique on the host.
2. **Add a device** (`POST /v1/devices`, signed in): `{ device_key, signature }`, signature over
   `lodestone-comms/v1 add-device <host url> <username> <device_key>` made by the **new** device. (Pairing
   UI to come; the existing device approves it.)
3. **Sign in** on a device:
   - `POST /v1/challenge` `{ device_key }` returns `{ nonce }` (valid for 2 minutes, single use).
   - `POST /v1/session` `{ device_key, nonce, signature }`, signature over
     `lodestone-comms/v1 login <host url> <nonce>`, returns `{ token, expires }`.
   - The token is 32 random bytes (base64url). The host stores only its SHA-256. Send it as
     `Authorization: Bearer <token>`. Sessions last 30 days.

## Spaces, channels and invites

| Request | Who | Does |
|---|---|---|
| `GET /v1/me` | signed in | your account, devices and spaces |
| `POST /v1/spaces` `{ name }` | signed in (or only host admins, if the host is set up that way) | creates a space you own, with a `general` channel |
| `GET /v1/spaces/{space}` | member | the space, its channels and members |
| `POST /v1/spaces/{space}/channels` `{ name }` | owner or admin | adds a text channel |
| `POST /v1/spaces/{space}/invites` `{ max_uses, expires_in }` | member | an invite code (`max_uses` 1 to 100, `expires_in` up to 30 days, in seconds) |
| `GET /v1/invites/{code}` | anyone with the code | the space's name and member count, to show before joining |
| `POST /v1/invites/{code}/accept` | signed in | joins the space |
| `DELETE /v1/spaces/{space}/members/{username}` | owner or admin (or yourself, to leave) | removes the member; `?ban=true` also stops them rejoining |

Roles: **owner** (one per space), **admin**, **member**. Names: 1 to 64 characters, no control characters.

Invite links have the form `<host url>/join/<code>`. Lodestone can send one to a friend from its friends
list: it opens that friend's Steam chat with the link ready to paste (Steam offers no way for other apps
to send messages), or copies it for Epic friends.

## End-to-end encryption (MLS)

- Each text channel is one MLS group; the channel ID is the MLS group ID. Ciphersuite:
  `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519`.
- Each device's MLS credential is a basic credential whose identity is `<username>:<device_key>` (a colon can't appear in either), signed
  with the device key. Clients check that the username and key match what the host lists for that member,
  and show a **safety number** that members can compare to make sure the host hasn't swapped a key.
- The host is MLS's **delivery service**: it doesn't parse MLS messages, it stores and forwards them in
  order.

### Key packages
- `POST /v1/keypackages` `{ key_packages: [base64, ...] }`: a device uploads a batch (up to 100 waiting).
- `GET /v1/keypackages`: how many of your device's key packages are still waiting.
- `POST /v1/keypackages/claim` `{ username }`: returns one key package per device of that user and
  removes them from the host (single use).

### Sending into a channel
`POST /v1/channels/{channel}/messages` `{ kind, epoch, body }`, members only.
- `kind`: `commit`, `proposal` or `application` (an encrypted chat message).
- `epoch`: the MLS epoch the sender is in.
- `body`: the MLS message, base64, at most 64 KiB.
- The host gives each accepted message a sequence number and sends it to members in that order.
- **Commits are ordered by the host:** a commit is only accepted if its `epoch` equals the channel's
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
- When someone joins a space, the host tells online members (`member_joined`). A member's client claims
  the newcomer's key packages, commits an Add to each channel group and sends the Welcome. If two clients
  try at once, the epoch rule lets one win; the other sees the member is already in.
- When someone leaves or is removed, the host stops sending them anything at once, and a member's client
  commits a Remove so their keys no longer work for new messages.

## Live connection

`GET /v1/live` upgrades to a WebSocket. The session token goes in the `Sec-WebSocket-Protocol` header as
`lodestone-comms.v1, <token>` (not in the URL, so it can't end up in logs). The host sends JSON events:

- `message` `{ channel, seq, kind, epoch, sender, sender_name, body, sent_at }` (`sender` is the device key)
- `welcome` `{ channel }` (collect it with `GET /v1/welcomes`)
- `member_joined` / `member_left` `{ space, username }`
- `channel_created` `{ space, channel, name }`

## Limits

- A device may have at most 5 sign-in challenges waiting; 60 messages a minute per device.
- 64 KiB per message body. Attachments come later, as encrypted uploads.

## Later (not in version 1 yet)

- Voice, video and screen share: the host forwards encrypted media between clients (an SFU) without
  decoding it. Media keys come from the channel's MLS group (the MLS exporter), SFrame-style.
- Encrypted attachments, reactions, message edits and deletes (as encrypted application messages).
- Device pairing UI and safety-number checks in the client.
