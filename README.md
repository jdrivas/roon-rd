# roon-rd - Roon Remote Display

A Rust-based Roon extension that provides both command-line querying and a web server API for accessing Roon playback information.

## Features

- **CLI Mode**: Query Roon information directly from the command line
- **Server Mode**: Run as a web server with REST API endpoints
- **Roon Extension**: Integrates with Roon Core as an authorized extension

## Prerequisites

- Rust toolchain (1.70 or later)
- Running Roon Core on your network
- Network access to Roon Core

## Installation

```bash
cd ~/Development/rust/roon-rd
cargo build --release
```

The compiled binary will be at `target/release/roon-rd`.

## Usage

### Command Line Query Mode

Query Roon status:
```bash
./target/release/roon-rd query status
```

Query available zones:
```bash
./target/release/roon-rd query zones
```

Query what's currently playing:
```bash
./target/release/roon-rd query now-playing
```

Enable verbose logging:
```bash
./target/release/roon-rd -v query status
```

### Server Mode

Start the web server (default port 3000):
```bash
./target/release/roon-rd server
```

Start on a custom port:
```bash
./target/release/roon-rd server --port 8080
```

### Web API Endpoints

When running in server mode, the following endpoints are available:

- `GET /` - Single page application for displaying zones and playback in a browser
- `GET /status` - Get Roon connection status
- `GET /zones` - Get list of available zones
- `GET /now-playing` - Get currently playing tracks

Example:
```bash
curl http://localhost:3000/status
```

## Authorization

On first run, you need to authorize the extension in Roon:

1. Start the application (either query or server mode)
2. Open Roon on your device
3. Go to **Settings** → **Extensions**
4. Find "Roon Remote Display" and click **Enable**

The authorization is saved and persists across runs.

## Development

Build in development mode:
```bash
cargo build
```

Run with cargo:
```bash
cargo run -- query status
cargo run -- server --port 3000
```

## Project Structure

```
roon-rd/
├── src/
│   ├── main.rs          # Entry point and CLI parsing
│   ├── cli/             # CLI query handlers
│   ├── server/          # Web server and API endpoints
│   └── roon/            # Roon API client wrapper
├── Cargo.toml
└── README.md
```

## License

MIT
