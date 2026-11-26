# Lodestone Beacon (in development)

Text chat for Lodestone, on servers you run yourself, end-to-end encrypted. Voice, video and screen
share come later.

- `server/`: the **server** (the program people run in Docker or Podman). One server holds many **communities**.
- `client/`: the **client core**: this device's key, MLS encryption (openmls) and the server connection.
  It will run inside Lodestone as a sandboxed plug-in; there's no interface yet.

**Status:** early development on the `comms` branch. Not ready to use.

- [PROTOCOL.md](PROTOCOL.md): how clients and the server talk, and exactly what the server can see.
- The server never sees message text. It stores and forwards MLS ciphertext, usernames, device public keys,
  community and channel names, and who is in which community.

## Run a server

With Docker or Podman, and a domain pointing at the machine (ports 80 and 443 open):

```bash
BEACON_DOMAIN=beacon.example.com docker compose up -d
docker compose logs beacon   # shows the one-time setup code for the first (admin) account
```

### Behind a reverse proxy (Cloudflare Tunnel, Zoraxy, Traefik, …)

Use `compose.proxy.yml`: Beacon listens on plain HTTP on the server and your proxy provides HTTPS.

It uses the official image from Docker Hub, `daigonstar/lodestonebeacon` (set `BEACON_VERSION` to pick a
version), so you only need the compose file. To build from source instead, swap its `image:` line for
`build: .` and run it from this folder.

1. Put `compose.proxy.yml` on the server and create `.env` beside it (or, in Portainer, paste it as a
   stack and set these as its environment variables):
   `BEACON_DOMAIN=beacon.yourdomain.com`, and for voice calls
   `BEACON_MEDIA_ADDR=home.yourdomain.com:7881,<server LAN IP>:7881` (see below).
2. `docker compose -f compose.proxy.yml up -d`, then `docker compose -f compose.proxy.yml logs beacon`
   for the one-time setup code.
3. **Zoraxy:** add a proxy rule for `beacon.yourdomain.com`. If Zoraxy runs in Docker (the usual way),
   point it at `lodestone-beacon:8080`: the container's own port, not 8095, with both containers on the
   same Docker network (the `networks` option in the file). Only a Zoraxy installed directly on the
   server uses `127.0.0.1:8095`. WebSockets must be allowed (Zoraxy does by default).
4. **Cloudflare Tunnel:** a public hostname `beacon.yourdomain.com` pointing at Zoraxy, like your other
   services. If Cloudflare's bot protection or WAF challenges requests, add a skip rule for this
   hostname: Beacon clients are apps, not browsers, and can't solve challenges.
5. Check it: `https://beacon.yourdomain.com/.well-known/lodestone-beacon` should show your address.
   A Cloudflare "error 521" there usually means the proxy is pointed at the wrong port.
6. In Lodestone → Beacon → **Set up a new server**, type `beacon.yourdomain.com` and the setup code.

Voice and video calls send media over UDP port 7881, which a Cloudflare Tunnel can't carry. Forward UDP
7881 on your router to the server and set `BEACON_MEDIA_ADDR`: first a name for your home internet
address that Cloudflare doesn't proxy (a grey-cloud record, such as one a DDNS container keeps current;
Beacon reads it when it starts, so restart it if your IP changes), then the server's LAN address for
people on your own network. Without it, voice is off and text chat still works through the tunnel.

The container runs as a non-root user with a read-only filesystem and no Linux capabilities; only its
data volume is writable.

To just try Beacon on one computer, the app's setup guide has a **Start it for me** button: it runs the server inside the app, on `127.0.0.1:8080` only (nothing outside this computer can reach it), with its data in Beacon's own data folder (`local-server`). Voice calls are off in that mode. It starts again whenever Beacon starts, once you've set it up.

Without containers (for development): `cargo run -p lodestone-beacon-server`, listening on port 8080.

| Setting | Default | What it does |
|---|---|---|
| `BEACON_PUBLIC_URL` | `http://localhost:8080` | the exact address clients use (`https://your.domain`); it's part of every signed sign-in and invite link, and clients check it when you type just the domain |
| `BEACON_BIND` | `0.0.0.0:8080` | where the program listens (put HTTPS in front of it) |
| `BEACON_DATA` | `./data` | where the database lives |
| `BEACON_ADMIN_ONLY_COMMUNITIES` | `false` | `true` lets only server admins create communities |
| `BEACON_MAX_FILE_MB` | `10` | largest attachment (files are encrypted before they reach the server) |
| `BEACON_COMMUNITY_QUOTA_MB` | `2048` | attachment space per community |
| `BEACON_MEDIA_ADDR` | (off) | the voice relay's address as clients reach it, `server:port` (UDP); voice is off without it. Several can be given, comma-separated: put the public one first and the LAN one second, so people on the server's own network can call even if the router can't loop back to its public address |
| `BEACON_MEDIA_BIND` | `0.0.0.0:7881` | where the voice relay listens (forward this UDP port on your router) |

## Tests

```bash
cargo test
```
