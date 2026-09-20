# Deployment

Kura is a single daemon over one SQLite database. Running it for a team
means three things the single-user default does not need: TLS in front,
bearer tokens for everyone, and something watching it.

## Install as a service

`./deploy/install.sh` builds the daemon and registers it as a background
service (launchd on macOS, systemd on Linux); `deploy/docker/` builds an
image and runs it with `docker compose`. See the repository's
`deploy/README.md` for flags, upgrades and uninstall.

## TLS

The daemon never terminates TLS itself. A reverse proxy does, so
certificates, renewal and cipher policy live in a tool built for them and
the daemon keeps binding a private address:

```bash
cd deploy/docker
KURA_PUBLIC_HOST=kura.example.com docker compose \
  -f docker-compose.yml -f docker-compose.tls.yml up -d --build
```

`docker-compose.tls.yml` removes the daemon's host port and publishes only
Caddy on 80/443 with automatic Let's Encrypt certificates; `Caddyfile`
keeps SSE streams unbuffered and forwards `X-Request-Id` into the daemon's
access log. TLS protects the transport, the daemon's auth protects the
data — a deployment reachable beyond loopback needs both.

## Multi-user

Every request runs behind bearer-token auth; a token resolves a tenant, and
every tenant-owned read and write is scoped by it (a route audit gate in
the test suite keeps new routes from opting out). Tokens are persisted, so
a restart keeps every active session; revocations stick.

## Observability

`GET /metrics` (bearer-authenticated) serves a Prometheus text exposition:
request latency by route template, LLM dispatch latency and token spend by
tenant, store lock wait by role, hook waterfall duration, and tool calls.
Every response carries an `x-request-id`, and the daemon writes one
`http.request` access-log line per request.

```yaml
scrape_configs:
  - job_name: kura
    authorization: { credentials: "<bearer token>" }
    static_configs: [{ targets: ["kura.example.com"] }]
    scheme: https
```

## Store

`store.readers` (or `KURA_STORE_READERS`) opens query-only reader
connections beside the single writer so reads run concurrently with writes
under WAL. `0` is the pre-pool single-connection behaviour, so the pool can
be enabled and rolled back by config alone.

## Backup and restore

Backups are online SQLite backups (`scripts/production/backup-test-state.sh`,
which validates the copy's schema version and integrity): they read through
the WAL while the daemon keeps writing, so no quiet window is needed. A
plain file copy of `daemon.sqlite` is **not** a backup — it silently loses
everything still in the WAL. Restore by opening the copy as a data
directory; migrations run on open, and the reader pool opens against it.
`kura daemon rehearse-upgrade` rehearses a migration on a snapshot before
activating it.
