# Deploy — InitConnection + coturn

Server side of RDOC-SACompanion. **Untested scaffold** — verify before relying on it.

## Setup

1. `cp .env.example .env` and fill:
   - `ROOM_AUTH_SECRET` — HMAC secret for room join tokens.
   - `TURN_SECRET` — coturn shared secret (≥32 chars).
   - `PUBLIC_HOST` — the server's FQDN (used for TLS SAN + `turns:` URL).
2. Put `TURN_SECRET` into `turnserver.conf` → `static-auth-secret=` (must match).
3. `docker compose up -d --build`.
   - `init` auto-generates `certs/init-cert.pem` + `init-key.pem` on first run and
     **prints the cert SHA-256** in its logs (`docker compose logs init`).
   - coturn reuses those certs for `turns://`.

## Client

```
SERVER=wss://<PUBLIC_HOST>:8080/ws
CERT_SHA256=<fingerprint from init logs>
ROOM=<room>
TOKEN=<run `init-connection mint <room>` with ROOM_AUTH_SECRET set>
```

## Firewall

- TCP `8080` — signaling (wss).
- TCP/UDP `3478`, TCP `5349` — coturn STUN/TURN(S).
- **UDP `49152-65535`** — TURN relay range (must be open).

## Notes

- coturn runs `network_mode: host` — TURN relay needs real source ports (docker
  NAT rewrite breaks ICE).
- Mint a room token: `ROOM_AUTH_SECRET=… docker compose exec init init-connection mint <room>`.
