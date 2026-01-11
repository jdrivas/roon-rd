# Build Notes for roon-rd

## Cross-Platform Builds

### macOS (Native)
```bash
cargo build --release
# Binary: target/release/roon-rd
```

### Windows 11
```bash
# One-time setup
rustup target add x86_64-pc-windows-gnu
brew install mingw-w64

# Build
cargo build --release --target x86_64-pc-windows-gnu
# Binary: target/x86_64-pc-windows-gnu/release/roon-rd.exe
```

**Status**: ✅ Working - 10MB executable ready to run on Windows 11

### iPad/iOS

**Requirements**:
- Full Xcode installation (not just Command Line Tools) - ~50GB
- iOS SDK (included with Xcode)
- Accept Xcode license: `sudo xcodebuild -license accept`

**Build steps**:
```bash
# One-time setup
rustup target add aarch64-apple-ios

# Build
cargo build --release --target aarch64-apple-ios
# Binary: target/aarch64-apple-ios/release/roon-rd
```

**Important Limitations**:
- iOS does NOT allow standalone executables like macOS/Windows
- Apps must be packaged as `.ipa` files
- Distribution options:
  1. App Store (requires Apple Developer account $99/year)
  2. TestFlight (requires Apple Developer account)
  3. Xcode development builds (7-day expiration)
  4. Enterprise distribution (requires enterprise account)

**Status**: ⚠️ Not pursued - requires Xcode installation

### Alternative for iPad: Web Interface

**Recommended approach** - No native app needed:

1. Run server on Mac/Windows/Linux:
   ```bash
   ./target/release/roon-rd server --port 3000
   ```

2. Find your Mac's IP address:
   ```bash
   ipconfig getifaddr en0   # WiFi
   ipconfig getifaddr en1   # Ethernet
   ```

3. Access from iPad Safari:
   ```
   http://<your-mac-ip>:3000
   ```

**Benefits**:
- Works immediately, no app installation
- Full-screen web app on iPad
- Responsive design already implemented
- Album art scales to 50% viewport width
- CORS enabled for cross-origin requests

**Deployment Options**:
- Run on always-on Mac Mini/iMac
- Run on Windows PC
- Deploy to Raspberry Pi
- Deploy to cloud server (AWS, DigitalOcean, etc.)

## Current Features
- CLI query mode: `roon-rd query status|zones|now-playing`
- Interactive mode: `roon-rd interactive`
- Web server mode: `roon-rd server --port 3000`
- Album art display (proactively cached)
- Full-width responsive layout
- Safari-compatible (CORS enabled)
- Connection status indicator
- Multi-zone support with zone selector

## Dependencies
- Rust 1.70+
- Roon Core on local network
- For Windows builds: mingw-w64 (via Homebrew on macOS)
- For iOS builds: Full Xcode installation
