# roon-rd - Roon Remote Display

A Rust-based Roon extension that provides command-line querying, interactive control, and a web-based single-page application for accessing and controlling Roon playback.

## Features

- **CLI Query Mode**: Query Roon information directly from the command line
- **Interactive CLI Mode**: Interactive shell for real-time Roon control
- **Server Mode**: Web server with REST API and browser-based SPA
- **Roon Extension**: Integrates with Roon Core as an authorized extension
- **Real-time Updates**: WebSocket support for live playback updates
- **Multi-zone Support**: Control and monitor multiple Roon zones
- **Queue Management**: View and interact with playback queues

## Prerequisites

- Rust toolchain (1.70 or later)
- Running Roon Core on your network
- Network access to Roon Core

## Installation

### From Source

```bash
git clone https://github.com/jdrivas/roon-rd.git
cd roon-rd
cargo build --release
```

The compiled binary will be at `target/release/roon-rd`.

### Pre-built Binaries

Download the latest release for your platform from the [Releases](https://github.com/jdrivas/roon-rd/releases) page:
- macOS ARM64 (Apple Silicon)
- macOS x64 (Intel)
- Windows x64

## Usage

### Command Line Query Mode

Query Roon information with single commands:

```bash
# Query Roon connection status
./roon-rd query status

# List available zones
./roon-rd query zones

# Show what's currently playing across all zones
./roon-rd query now-playing

# Enable verbose logging
./roon-rd -v query status
```

**Query Mode Features:**
- Single-shot queries that exit immediately
- Silent by default (unless `-v` flag is used)
- JSON output suitable for scripting
- No persistent connection

### Interactive Mode

Launch an interactive shell for real-time Roon control:

```bash
./roon-rd interactive
```

**Interactive Mode Features:**
- Command history with readline support
- Real-time query capabilities
- Persistent connection to Roon Core
- Available commands:
  - `status` - Show connection status
  - `zones` - List available zones
  - `now-playing` - Show current playback
  - `help` - Show available commands
  - `quit` or `exit` - Exit interactive mode

Example session:
```
> status
Connected to Roon Core: My Core v2.0
> zones
Zone: Living Room (playing)
Zone: Bedroom (stopped)
> now-playing
Living Room: Artist - Track Name
```

### Server Mode

Start the web server with integrated SPA:

```bash
# Start server on default port 3000
./roon-rd server

# Start on custom port
./roon-rd server --port 8080

# With verbose logging
./roon-rd -v server
```

**Server Mode Features:**
- RESTful API for programmatic access
- WebSocket endpoint for real-time updates
- Built-in Single Page Application (SPA)
- Automatic album art caching
- CORS enabled for cross-origin requests

### Single Page Application (SPA)

The server includes a modern web-based interface accessible at `http://localhost:3000`:

**SPA Features:**
- **Real-time Display**: Live playback information with automatic updates
- **Multi-zone Support**: View and control all Roon zones simultaneously
- **Zone Selector**: Filter by specific zone or view all zones
- **Playback Controls**: Play, pause, previous, next track controls
- **Progress Bar**: Visual playback progress with seek support
- **Queue View**: Click album art to view and navigate playback queue
- **Album Art**: High-quality album artwork display
- **Fullscreen Mode**: Optimized for dedicated displays
- **Responsive Design**: Adapts to different screen sizes
- **Dark Theme**: Easy-on-the-eyes interface design
- **Custom Icons**: Zone-specific 3D icons for recognized devices

**Large Screen Mode** (≥1500px):
- Enlarged fonts for better visibility
- Optimized layout for dedicated displays
- Perfect for wall-mounted tablets or TV displays

## REST API Endpoints

When running in server mode, the following endpoints are available:

### Public Endpoints

- `GET /` - Serve the Single Page Application
- `GET /status` - Get Roon Core connection status
- `GET /zones` - Get list of all available zones with device info
- `GET /now-playing` - Get currently playing tracks across all zones
- `GET /queue/:zone_id` - Get playback queue for a specific zone
- `GET /image/:image_key` - Get album art image by Roon image key
- `GET /ws` - WebSocket endpoint for real-time zone updates

### Control Endpoints

- `POST /control/:zone_id` - Control playback (play, pause, stop, previous, next)
- `POST /seek/:zone_id` - Seek to position in current track
- `POST /mute/:zone_id` - Toggle mute for zone
- `POST /play-from-queue/:zone_id` - Play specific item from queue
- `POST /reconnect` - Reconnect to Roon Core

### API Examples

```bash
# Get connection status
curl http://localhost:3000/status

# List all zones
curl http://localhost:3000/zones

# Get now playing information
curl http://localhost:3000/now-playing

# Get queue for a specific zone
curl http://localhost:3000/queue/ZONE_ID

# Control playback (play/pause/stop/previous/next)
curl -X POST http://localhost:3000/control/ZONE_ID \
  -H "Content-Type: application/json" \
  -d '{"action": "play"}'

# Seek to 60 seconds into current track
curl -X POST http://localhost:3000/seek/ZONE_ID \
  -H "Content-Type: application/json" \
  -d '{"position": 60}'

# Reconnect to Roon Core
curl -X POST http://localhost:3000/reconnect
```

### WebSocket Updates

Connect to `ws://localhost:3000/ws` to receive real-time zone updates:

```javascript
const ws = new WebSocket('ws://localhost:3000/ws');
ws.onmessage = (event) => {
  const zones = JSON.parse(event.data);
  console.log('Zone update:', zones);
};
```

## Authorization

On first run, you need to authorize the extension in Roon:

1. Start the application (any mode: query, interactive, or server)
2. The app will display: "Please authorize this extension in Roon Settings > Extensions"
3. Open Roon on your device
4. Go to **Settings** → **Extensions**
5. Find "Roon Remote Display" and click **Enable**

The authorization token is saved to `~/.roon_token` and persists across runs.

## Configuration

### Server Configuration
- **Port**: Set via `--port` flag (default: 3000)
- **Logging**: Enable with `-v` or `--verbose` flag

### Token Storage
- Token file: `~/.roon_token`
- Automatically created on first authorization
- Reused for subsequent connections

## Development

### Building

```bash
# Development build
cargo build

# Release build (optimized)
cargo build --release
```

### Running from Source

```bash
# Query mode
cargo run -- query status

# Interactive mode
cargo run -- interactive

# Server mode
cargo run -- server --port 3000

# With verbose logging
cargo run -- -v server
```

### Testing

```bash
# Run tests
cargo test

# Run with logging
cargo test -- --nocapture
```

## Project Structure

```
roon-rd/
├── src/
│   ├── main.rs          # Entry point and CLI parsing
│   ├── cli/             # CLI and interactive mode handlers
│   │   └── mod.rs       # Query and interactive command processing
│   ├── server/          # Web server, API, and SPA
│   │   └── mod.rs       # HTTP handlers, WebSocket, embedded HTML/CSS/JS
│   └── roon/            # Roon API client wrapper
│       └── mod.rs       # Wrapper for roon-api crate with state management
├── Cargo.toml           # Dependencies and project metadata
├── Makefile             # Build automation for multi-platform releases
└── README.md
```

## Building Multi-Platform Releases

The project includes a Makefile for building releases across multiple platforms:

```bash
# Build all platform releases
make release

# Build specific platforms
make mac              # Both macOS variants
make mac-arm64        # Apple Silicon only
make mac-x64          # Intel Mac only
make windows          # Windows x64
make linux            # Linux x64 (requires cross)

# Create GitHub release with binaries
make github-release

# Install all Rust targets
make install-targets
```

## How It Works

### Architecture

1. **Roon API Client** (`src/roon/mod.rs`)
   - Wrapper around the Roon API for control and metadata
   - Connects to Roon Core via network discovery
   - Manages authentication and service subscriptions
   - Handles transport control, image fetching, and browsing
   - Maintains persistent connection to Roon Core

2. **CLI Modes** (`src/cli/mod.rs`)
   - **Query Mode**: One-shot queries with immediate exit
   - **Interactive Mode**: Persistent shell with command history
   - Both modes use the same Roon client underneath

3. **Web Server** (`src/server/mod.rs`)
   - Axum-based async HTTP server
   - Embeds SPA as compiled binary (no external files needed)
   - Maintains zone subscription for real-time updates
   - Caches album art for performance
   - Broadcasts updates via WebSocket

4. **SPA** (embedded in `src/server/mod.rs`)
   - Pure HTML/CSS/JavaScript (no framework dependencies)
   - WebSocket for real-time zone updates
   - Responsive design with media queries
   - Queue overlay with blur effects
   - Progress bars with seek functionality

### Data Flow

```
Roon Core <--Roon API--> Roon Client <---> CLI/Server
                                              │
                                              ├─> REST API
                                              ├─> WebSocket
                                              └─> SPA (HTML/CSS/JS)
```

Note: This application uses the Roon API for control and metadata, not RAAT.
RAAT (Roon Advanced Audio Transport) is Roon's proprietary audio streaming
protocol used between Roon Core and audio endpoints. This application controls
playback but does not handle audio transport.

### Zone Updates

1. Server subscribes to Roon transport service
2. Roon Core pushes zone state changes
3. Server broadcasts updates via WebSocket
4. SPA receives updates and updates UI
5. No polling required - all updates are push-based

## Version History

- **v1.3.2** - Queue overlay improvements, loading text fix
- **v1.3.1** - UI improvements, version display
- **v1.3.0** - Version bump
- **v1.2.1** - Multi-zone queue support
- Earlier versions - See [releases](https://github.com/jdrivas/roon-rd/releases)

## License

MIT

## Contributing

Contributions are welcome! Please feel free to submit issues or pull requests.

## Acknowledgments

Built with:
- [roon-api](https://github.com/TheAppgineer/rust-roon-api) - Rust Roon API bindings
- [Axum](https://github.com/tokio-rs/axum) - Web framework
- [Tokio](https://tokio.rs/) - Async runtime
- [Clap](https://github.com/clap-rs/clap) - CLI parsing
