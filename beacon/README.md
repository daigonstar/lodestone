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

1. Copy this folder to the server and create `.env` beside it with `BEACON_DOMAIN=beacon.yourdomain.com`.
2. `docker compose -f compose.proxy.yml up -d --build`, then `docker compose -f compose.proxy.yml logs beacon`
   for the one-time setup code.
3. **Zoraxy:** add a proxy rule for `beacon.yourdomain.com` to `127.0.0.1:8095` (or `lodestone-beacon:8080`
   if you use the shared-network option in the file). WebSockets must be allowed (Zoraxy does by default).
4. **Cloudflare Tunnel:** a public hostname `beacon.yourdomain.com` pointing at Zoraxy, like your other
   services. If Cloudflare's bot protection or WAF challenges requests, add a skip rule for this
   hostname: Beacon clients are apps, not browsers, and can't solve challenges.
5. Check it: `https://beacon.yourdomain.com/.well-known/lodestone-beacon` should show your address.
6. In Lodestone → Beacon → **Set up a new server**, type `beacon.yourdomain.com` and the setup code.

Voice and video (later) send media over UDP, which a Cloudflare Tunnel can't carry; they'll need a UDP
port forwarded to the server. Text chat works through the tunnel as it is.

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
