---
description: How to build roon-rd for development and release
---

## Development Build

1. Run `cargo build` from the project root

## Release Build (optimized)

1. Run `cargo build --release` from the project root
2. Binary output: `target/release/roon-rd`

## Multi-Platform Release Builds

1. Install all Rust targets: `make install-targets`
2. Build all platforms: `make release`
   - Or specific: `make mac-arm64`, `make mac-x64`, `make windows`
   - Linux requires Docker + `cross`: `make linux`
3. Binaries output to `release/v<VERSION>/`

## GitHub Release

1. Ensure all platform binaries are built: `make release`
2. Create release: `make github-release`
   - Requires `gh` CLI authenticated (`gh auth status`)
   - Auto-generates release notes
   - Uploads all available binaries + install.ps1

## Version Bump

1. Update version in `Cargo.toml` (line 3: `version = "x.y.z"`)
2. The Makefile reads version from Cargo.toml automatically
3. build.rs embeds git hash into BUILD_HASH env var (dirty builds include timestamp)

## Cross-Compilation Notes

- **Windows**: Requires `mingw-w64` (`brew install mingw-w64`)
- **Linux**: Requires Docker + `cross` (`cargo install cross`)
- **macOS x64** (from ARM): Native cross-compile via rustup target
