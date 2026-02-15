---
description: Project architecture and code organization reference for roon-rd
---

## Source Modules

| Module | File | Size | Purpose |
|--------|------|------|---------|
| main | `src/main.rs` | ~185 lines | CLI parsing (clap), log setup, dispatches to modes |
| cli | `src/cli/mod.rs` | ~113KB | Query, interactive, TUI mode handlers |
| server | `src/server/mod.rs` | ~103KB | Axum web server, REST API, WebSocket, embedded SPA |
| roon | `src/roon/mod.rs` | ~62KB | RoonClient wrapper around rust-roon-api |
| tui | `src/tui/mod.rs` | ~78KB | Ratatui terminal UI |
| dcs | `src/dcs/mod.rs` | ~29KB | dCS audio device HTTP API integration |
| upnp | `src/upnp/mod.rs` | ~15KB | UPnP/SSDP discovery and control |

## Key Architectural Decisions

- **Shared state**: `Arc<Mutex<RoonClient>>` passed to all modes
- **SPA**: Embedded as string constants in `server/mod.rs` (no separate frontend build)
- **Zone updates**: Push-based via WebSocket (no polling)
- **Queue**: On-demand single-zone subscription ("Option C") — see TODO.md for alternatives
- **Roon API**: Pinned to specific git rev of rust-roon-api (33516cc)
- **mDNS**: Registers `roon-rd.local` hostname automatically in server mode

## Data Flow

```
Roon Core <--Roon API--> RoonClient (src/roon/) <--Arc<Mutex>--> CLI/Server/TUI
                                                                      |
                                                                      +-> REST API (/zones, /now-playing, etc.)
                                                                      +-> WebSocket (/ws) — push zone updates
                                                                      +-> Embedded SPA (/)
```

## When Editing

- **server/mod.rs** contains both Rust server code AND the entire SPA (HTML/CSS/JS as string literals). The SPA portion is large.
- **cli/mod.rs** is the largest file — handles query, interactive, AND TUI dispatch. Has .bak files from previous refactoring.
- **roon/mod.rs** wraps the external rust-roon-api crate. Queue subscription logic is here.
- **dcs/mod.rs** talks to dCS hardware via HTTP. Known issue: `get_audio_format()` makes 3 separate calls and can return stale data.
