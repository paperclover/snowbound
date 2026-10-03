# snowbound-relay

The relay Live Share meets through when two Snowbounds aren't on one network. Peers join a
room named by a tag (a hash of the notebook's secret, or a code's number), and the relay
passes their messages between them. Every message after the opening is sealed end to end
and numbered inside the seal, so the relay can't read, alter, drop or reorder one unseen;
it learns who talks to whom, when, and how much. `src/lib.rs` describes the protocol.

One static Linux executable with no configuration file and nothing on disk. It speaks plain
HTTP and WebSocket; a proxy in front of it terminates TLS.

## Build

```sh
python3 tools/release_relay.py   # target/relay/snowbound-{relay,site}-linux-{x86_64,aarch64}
```

It links with Rust's own lld against Rust's own musl, so it needs only `rustup`. The folder
it makes holds the executables, `SHA256SUMS`, this README and both systemd units.

## Deploy

```sh
scp target/relay/snowbound-relay-linux-x86_64 vps:/tmp/snowbound-relay
scp target/relay/snowbound-relay.service vps:/tmp/
ssh vps
sudo install -m 755 /tmp/snowbound-relay /usr/local/bin/snowbound-relay
sudo install -m 644 /tmp/snowbound-relay.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now snowbound-relay
curl -s http://127.0.0.1:23592/health     # {"rooms":0,"peers":0,"connections":1,"seconds":3}
```

Then point a name at the server and put a TLS proxy in front. Caddy fetches its own
certificate and passes WebSocket upgrades through as they are:

```text
live.example.net {
    reverse_proxy 127.0.0.1:23592
}
```

nginx, with a certificate from certbot:

```nginx
server {
    listen 443 ssl;
    server_name live.example.net;
    ssl_certificate     /etc/letsencrypt/live/live.example.net/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/live.example.net/privkey.pem;
    location / {
        proxy_pass http://127.0.0.1:23592;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        # Replaced, not appended to, so a client can't name its own address.
        proxy_set_header X-Forwarded-For $remote_addr;
        proxy_read_timeout 1h;
    }
}
```

The app uses `wss://relay.snowbound.paperclover.net` unless Options ▸ Sync & Storage ▸ Live
Share names another relay.

`--trust-forwarded true`, as the unit sets it, counts each peer by the last
`X-Forwarded-For` entry, the one the proxy added. Without a proxy, leave it off: a client
could otherwise claim any address. Listening on a public address without TLS works but lets
anyone on the path see room tags and nameplates.

## The site: snowbound.paperclover.net

`snowbound-site` serves the hosted web build's folder, and for a path that is a Live Share
code (`/7KQ-4MZ-9XR`, read as loosely as the app reads one) a page that opens it with
`snowbound://join/<code>`, and in the web build where `--web` names where (once the web build
joins shares). Anything else under the folder is a file; `/` is its `index.html`. A build's
module and JavaScript sit in `b/<hash>/` and are cached for good; `index.html` and the
codes' pages are checked on every load, and the rest (fonts, dictionaries) for a day. It
holds no state.

`python3 tools/release_web.py --deploy` builds the web app and the site for the VPS's
architecture, copies the build into `~/snowbound-web/site/` (keeping the last three builds'
folders for pages still running them) and the binary to `~/snowbound-web/snowbound-site`,
and restarts pm2's `snowbound-site` only when the binary changed. The first time:

```sh
ssh vps
mkdir -p ~/snowbound-web/site
# after the first `release_web.py --deploy` has put the binary there:
pm2 start ~/snowbound-web/snowbound-site --name snowbound-site -- \
    --listen 127.0.0.1:23593 --root "$HOME/snowbound-web/site"
pm2 save
```

Caddy in front:

```text
snowbound.paperclover.net {
    encode gzip
    reverse_proxy 127.0.0.1:23593
}
```

`snowbound-site.service` runs it under systemd instead (`--root /srv/snowbound/web`).
`snowbound-site --help` lists the options, each also `SNOWBOUND_SITE_<OPTION>`.

## Options

`snowbound-relay --help` lists every option and its default. Each can also be set in the
unit's environment as `SNOWBOUND_RELAY_<OPTION>` (`SNOWBOUND_RELAY_MAX_ROOMS=64`).

Memory stays under about `max-connections × (max-message + queue)` plus two small thread
stacks per connection: some 320 MiB at the defaults, and a few MB in use by a handful of
people. The unit caps it at 512 MiB.

## Abuse

- **Wrong codes.** A peer joining a code's room hears only the room's owner until the owner
  tells the relay it met it (`met`), so the relay never needs the code. A peer the owner
  says `failed`, one that leaves first, and one silent past `--pending` each count a wrong
  code against its address (an IPv4 address or an IPv6 /64) and against the code. Ten in a
  minute lock the address out for a minute, then two, four, up to an hour, forgotten after
  a quiet hour; tries still waiting count toward the ten, so they can't run side by side.
  Five burn the code: it admits no one new, and the owner makes another.
- **Load.** Joins per address and per room are rate limited; connections in all, per
  address, rooms, and peers per room are capped; a room's bytes per second are throttled by
  slowing its senders; a message over `--max-message` or a peer too slow to take what waits
  for it (`--queue`) is hung up on; a silent connection closes after `--idle`.

The relay logs lockouts and burned codes to stderr (`journalctl -u snowbound-relay`), and
nothing else.
