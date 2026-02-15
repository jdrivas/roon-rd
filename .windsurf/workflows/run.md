---
description: How to run roon-rd in its various modes
---

## Prerequisites

- Roon Core running on the local network
- First run requires authorization in Roon Settings > Extensions > "Roon Remote Display"
- Auth token saved to `~/.roon_token`

## Query Mode (one-shot)

```bash
cargo run -- query status
cargo run -- query zones
cargo run -- query now-playing
```

## Interactive Mode (readline shell)

```bash
cargo run -- interactive
```

Commands: `status`, `zones`, `now-playing`, `play <zone>`, `pause <zone>`, `help`, `quit`

## TUI Mode (ratatui terminal UI)

```bash
cargo run -- tui
```

## Server Mode (web server + SPA)

```bash
# Default port 3000
cargo run -- server

# Custom port
cargo run -- server --port 8080

# With logging
cargo run -- --log-level info server
```

Access at: `http://localhost:3000` or `http://roon-rd.local:3000`

## Common Flags

- `--log-level <trace|debug|info|warn|error|off>` — Set log verbosity
- `--log-time <local|utc>` — Timestamp format
- `--upnp-only` — Skip Roon connection (UPnP/dCS only)

## Running Release Binary

```bash
./target/release/roon-rd server --port 3000
./target/release/roon-rd --log-level debug server
```
