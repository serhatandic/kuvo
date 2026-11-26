# Kuvo Link relay

Forwards encrypted traffic between box peers and between paired clients and
boxes. The relay cannot read the traffic. It also serves signed setup lease
requests at `POST /setup-lease`, keeping the main-computer lease, authorized
roster and Work Queue number floor in `DATA_DIR/setup-leases`.
APNs and push registrations remain for installed-phone compatibility.
See the [Kuvo Link protocol](../protocol/PROTOCOL.md).

## Run it

```sh
npm ci
npm start          # listens on 8790
npm test
npm run typecheck
```

Settings come from the environment:

| Variable | |
| --- | --- |
| `PORT`, `HOST` | Where to listen (`8790`, `0.0.0.0`) |
| `DATA_DIR` | Private data root for setup leases and optional APNs registrations (`./data`) |
| `TRUST_PROXY` | `1` behind a proxy on the same host, so limits per address use `X-Forwarded-For` |
| `APNS_KEY_ID`, `APNS_TEAM_ID` | The APNs key's ID and the Apple Developer team |
| `APNS_KEY` or `APNS_KEY_FILE` | The key (a `.p8` file's contents) or its path |

APNs is optional. Without a key the service still relays and handles leases and answers pushes with "no phone".

## Logs

It logs counts only, never a token, an address or anything sent: once an hour, how many devices
are connected, how many phones are registered, and how that hour's pushes went (sent, no phone
registered there, not one of its senders, refused by Apple, with Apple's reasons); and the first
time Apple gives each reason for refusing one. A phone that names the wrong environment for its
token (a build signed for development that says production) gets its pushes from Apple's other
host, which the relay then keeps for it, and logs that it did.

## Deploy

`docker compose up -d --build` runs it on `127.0.0.1:8790`, for a proxy in front that ends
TLS, passes WebSocket upgrades and `/setup-lease` through, and sets `X-Forwarded-For`, so devices connect to
`wss://<your host>/`. Put the APNs key in `apns.p8` beside `docker-compose.yml`, readable by
the container's user (uid 1000), and its ID and team in `.env` (see `.env.example`). An empty
`apns.p8` runs it without pushes.

The deployment configuration points to `wss://kuvolink.porigam.com.tr/`, behind
`/root/projects/edge-proxy` with no published port: `.env` adds
`COMPOSE_FILE=docker-compose.yml:docker-compose.vps.yml`, and nginx reaches it on the
`edge-kuvolink` network as `kuvolink-http:8790`.

The recorded deployment checkout is `/root/projects/kuvo-link/relay`. Run this from that folder to redeploy the relay service.

```sh
docker compose build relay && docker compose up -d --no-deps --force-recreate --wait relay
```

## Limits

The relay checks each connection every 30 seconds. A connection that has sent neither a pong
nor a message for at least 90 seconds is dropped at the next check. A connection whose reads the
relay itself has paused is not expired; its activity window begins again when reads resume.

A device sending faster than 500 messages a second on average, or faster than the device it
sends to takes them in, is held back (the relay stops reading from it for a moment) rather
than cut off; one that keeps on regardless is closed (4003). A device that takes nothing in
while 8 MB waits for it is dropped. One address gets 64 connections at once (4005). A push
registration lasts 90 days after the phone last sent it. A device is told once when a device it sent to leaves
(another connection took its address, or it closed); each connection keeps track of at most
1,024 that sent to it and 256 it sent to, and past that those devices just aren't told.
