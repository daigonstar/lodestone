# Lodestone Comms (in development)

Text chat for Lodestone, on servers you host yourself, end-to-end encrypted. Voice, video and screen
share come later.

- `server/`: the **host** (the server program people run in Docker or Podman).
- `client/`: the **client core**: this device's key, MLS encryption (openmls) and the host connection.
  It will run inside Lodestone as a sandboxed plug-in; there's no interface yet.

**Status:** early development on the `comms` branch. Not ready to use.

- [PROTOCOL.md](PROTOCOL.md): how clients and the host talk, and exactly what the host can see.
- The host never sees message text. It stores and forwards MLS ciphertext, usernames, device public keys,
  server and channel names, and who is in which server.

## Run a host

With Docker or Podman, and a domain pointing at the machine (ports 80 and 443 open):

```bash
COMMS_DOMAIN=comms.example.com docker compose up -d
docker compose logs comms   # shows the one-time setup code for the first (admin) account
```

### Behind a reverse proxy (Cloudflare Tunnel, Zoraxy, Traefik, …)

Use `compose.proxy.yml`: Comms listens on plain HTTP on the server and your proxy provides HTTPS.

1. Copy this folder to the server and create `.env` beside it with `COMMS_DOMAIN=comms.yourdomain.com`.
2. `docker compose -f compose.proxy.yml up -d --build`, then `docker compose -f compose.proxy.yml logs comms`
   for the one-time setup code.
3. **Zoraxy:** add a proxy rule for `comms.yourdomain.com` to `127.0.0.1:8095` (or `lodestone-comms:8080`
   if you use the shared-network option in the file). WebSockets must be allowed (Zoraxy does by default).
4. **Cloudflare Tunnel:** a public hostname `comms.yourdomain.com` pointing at Zoraxy, like your other
   services. If Cloudflare's bot protection or WAF challenges requests, add a skip rule for this
   hostname: Comms clients are apps, not browsers, and can't solve challenges.
5. Check it: `https://comms.yourdomain.com/.well-known/lodestone-comms` should show your address.
6. In Lodestone → Comms → **Set up a new host**, type `comms.yourdomain.com` and the setup code.

Voice and video (later) send media over UDP, which a Cloudflare Tunnel can't carry; they'll need a UDP
port forwarded to the server. Text chat works through the tunnel as it is.

The container runs as a non-root user with a read-only filesystem and no Linux capabilities; only its
data volume is writable.

Without containers (for development): `cargo run -p lodestone-comms-server`, listening on port 8080.

| Setting | Default | What it does |
|---|---|---|
| `COMMS_PUBLIC_URL` | `http://localhost:8080` | the exact address clients use (`https://your.domain`); it's part of every signed sign-in and invite link, and clients check it when you type just the domain |
| `COMMS_BIND` | `0.0.0.0:8080` | where the program listens (put HTTPS in front of it) |
| `COMMS_DATA` | `./data` | where the database lives |
| `COMMS_ADMIN_ONLY_SPACES` | `false` | `true` lets only host admins create servers |
| `COMMS_MAX_FILE_MB` | `10` | largest attachment (files are encrypted before they reach the host) |
| `COMMS_SPACE_QUOTA_MB` | `2048` | attachment space per server |

## Tests

```bash
cargo test
```
