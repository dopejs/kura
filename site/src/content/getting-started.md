# Getting Started

Kura is a **personal agent OS**: a local Rust daemon (the control
plane) plus thin clients — a full-screen terminal UI (`kura-tui`), a React
web shell, and chat-channel connectors. The daemon owns runtime state,
provider dispatch, policy gates, memory, and event fan-out; clients are
thin consumers of its HTTP API.

## Install

One line (macOS and Linux, arm64 + x86_64):

```bash
curl -fsSL https://kura.dopejs.com/install.sh | sh
```

The installer detects your platform, downloads the latest
[GitHub Release](https://github.com/dopejs/kura/releases), verifies
its SHA-256 against the release's `SHA256SUMS`, and installs `kura` (the
daemon) and `kura-tui` (the terminal client) into `~/.local/bin` or
`/usr/local/bin`.

Pin a version or destination with:

```bash
KURA_VERSION=v0.3.0 KURA_INSTALL_DIR=~/bin sh -c "$(curl -fsSL https://kura.dopejs.com/install.sh)"
```

Prefer manual? Grab a tarball from the releases page:

```bash
curl -LO https://github.com/dopejs/kura/releases/latest/download/kura-0.3.0-aarch64-apple-darwin.tar.gz
tar xzf kura-0.3.0-aarch64-apple-darwin.tar.gz
sudo install -m 755 kura-0.3.0-aarch64-apple-darwin/{kura,kura-tui} /usr/local/bin/
```

## Build from source

Requirements: Rust (1.85+), pnpm (for the web client).

```bash
git clone https://github.com/dopejs/kura
cd kura

# Daemon + TUI
make daemon-build                 # emits crates/target/release/kura
cd crates && cargo build --release -p kura-tui  # emits target/release/kura-tui

# Web client + SDK
pnpm install
pnpm build:clients
```

## Run the daemon

The `kura` CLI manages everything:

```bash
kura daemon start                 # background daemon: pidfile + logfile
kura daemon status                # pid, health, version
kura daemon stop                  # graceful stop
kura daemon run                   # foreground (for services/systemd)

kura tui                          # terminal client
kura web                          # serve + open the web operator shell
kura config show                  # effective configuration
kura config set llm.defaultProvider claude_code_cli
kura config edit                  # $EDITOR on config.json (validated)
```

Kura has two environments, selected by `KURA_ENV`:

| Mode | Data dir | Bind address | Command |
|------|----------|--------------|---------|
| prod (release default) | `~/.kura` | `127.0.0.1:19191` | `kura daemon start` |
| test | `~/.kura-test` | `127.0.0.1:19192` | `KURA_ENV=test kura daemon start` |

From a source checkout, the Make targets wrap the same thing and default
to the **test** environment (the safe development default):

```bash
make daemon-run-test              # test env
make daemon-test-status           # health check (GET /healthz)
make daemon-run-test-live         # test env with Discord enabled
```

Live connectors (Discord etc.) are **disabled by default** everywhere
until you enable them in config.

## First conversation

The daemon always ships a deterministic `echo` provider, so you can talk
to it before configuring any model:

```bash
curl -s http://127.0.0.1:19191/v1/chat/query \
  -H 'content-type: application/json' \
  -d '{"query": "hello", "provider": "echo"}'
```

Then configure a real provider (Claude CLI, Codex CLI, or any
OpenAI-compatible endpoint) — see **Configuration**.

## Terminal UI

```bash
kura tui                          # full-screen Claude-Code-style client
```

The TUI includes a live daemon event stream viewer (`/events`).
