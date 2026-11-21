# Kuvo Link relay

Passes encrypted messages between Kuvo Code on a phone and the Macs and machines it's
paired with, and sends their push notifications. It can't read either: everything is
encrypted end to end, and it keeps no content, only where to send each phone's pushes
and from whom. See the [Kuvo Link protocol](../protocol/PROTOCOL.md).

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
| `DATA_DIR` | Where phones' push registrations are kept (`./data`) |
| `TRUST_PROXY` | `1` behind a proxy on the same host, so limits per address use `X-Forwarded-For` |
| `APNS_KEY_ID`, `APNS_TEAM_ID` | The APNs key's ID and the Apple Developer team |
| `APNS_KEY` or `APNS_KEY_FILE` | The key (a `.p8` file's contents) or its path |

Without an APNs key it relays as usual and answers pushes with "no phone".

## Logs

It logs counts only, never a token, an address or anything sent: once an hour, how many devices
are connected, how many phones are registered, and how that hour's pushes went (sent, no phone
registered there, not one of its senders, refused by Apple, with Apple's reasons); and the first
time Apple gives each reason for refusing one. A phone that names the wrong environment for its
token (a build signed for development that says production) gets its pushes from Apple's other
host, which the relay then keeps for it, and logs that it did.

## Deploy

`docker compose up -d --build` runs it on `127.0.0.1:8790`, for a proxy in front that ends
TLS, passes WebSocket upgrades through and sets `X-Forwarded-For`, so devices connect to
`wss://<your host>/`. Put the APNs key in `apns.p8` beside `docker-compose.yml`, readable by
the container's user (uid 1000), and its ID and team in `.env` (see `.env.example`). An empty
`apns.p8` runs it without pushes.

On the porigam VPS it runs at `wss://kuvolink.porigam.com.tr/`, behind
`/root/projects/edge-proxy` with no published port: `.env` adds
`COMPOSE_FILE=docker-compose.yml:docker-compose.vps.yml`, and nginx reaches it on the
`edge-kuvolink` network as `kuvolink-http:8790`.

The running checkout on Hetzner is `/root/projects/kuvo-link/relay`. Run this from that folder to redeploy the relay service.

```sh
docker compose build relay && docker compose up -d --no-deps --force-recreate --wait relay
```

## Limits

A device sending faster than 500 messages a second on average, or faster than the device it
sends to takes them in, is held back (the relay stops reading from it for a moment) rather
than cut off; one that keeps on regardless is closed (4003). A device that takes nothing in
while 8 MB waits for it is dropped. One address gets 64 connections at once (4005). A push
registration lasts 90 days after the phone last sent it. A device is told once when a device it sent to leaves
(another connection took its address, or it closed); each connection keeps track of at most
1,024 that sent to it and 256 it sent to, and past that those devices just aren't told.
