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

Without containers (for development): `cargo run -p lodestone-comms-server`, listening on port 8080.

| Setting | Default | What it does |
|---|---|---|
| `COMMS_PUBLIC_URL` | `http://localhost:8080` | the address clients use; it's part of every signed sign-in |
| `COMMS_BIND` | `0.0.0.0:8080` | where the program listens (put HTTPS in front of it) |
| `COMMS_DATA` | `./data` | where the database lives |
| `COMMS_ADMIN_ONLY_SPACES` | `false` | `true` lets only host admins create servers |

## Tests

```bash
cargo test
```
