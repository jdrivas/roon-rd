# Makefile for roon-rd - Roon Remote Display
# Builds release binaries for macOS, Linux, and Windows

# Project info
BINARY_NAME := roon-rd
VERSION := $(shell grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
RELEASE_DIR := release/v$(VERSION)

# Targets
TARGET_MAC_ARM64 := aarch64-apple-darwin
TARGET_MAC_X64 := x86_64-apple-darwin
TARGET_LINUX_X64 := x86_64-unknown-linux-gnu
TARGET_WINDOWS_X64 := x86_64-pc-windows-gnu

# Output binaries
BIN_MAC_ARM64 := $(RELEASE_DIR)/$(BINARY_NAME)-macos-arm64
BIN_MAC_X64 := $(RELEASE_DIR)/$(BINARY_NAME)-macos-x64
BIN_LINUX_X64 := $(RELEASE_DIR)/$(BINARY_NAME)-linux-x64
BIN_WINDOWS_X64 := $(RELEASE_DIR)/$(BINARY_NAME)-windows-x64.exe

.PHONY: all clean release mac linux windows install-targets github-release help

# Default target
all: help

help:
	@echo "Roon Remote Display - Build System"
	@echo "Version: $(VERSION)"
	@echo ""
	@echo "Available targets:"
	@echo "  make release         - Build all platform releases"
	@echo "  make mac             - Build both macOS releases (ARM64 + x64)"
	@echo "  make mac-arm64       - Build macOS ARM64 release"
	@echo "  make mac-x64         - Build macOS x64 release"
	@echo "  make linux           - Build Linux x64 release"
	@echo "  make windows         - Build Windows x64 release"
	@echo "  make install-targets - Install all required Rust targets"
	@echo "  make github-release  - Create GitHub release and upload binaries"
	@echo "  make clean           - Clean build artifacts"
	@echo "  make help            - Show this help message"

# Build all releases
release: mac linux windows
	@echo "✓ All releases built successfully in $(RELEASE_DIR)/"
	@ls -lh $(RELEASE_DIR)/

# Build all macOS releases
mac: mac-arm64 mac-x64

# Build macOS ARM64 (Apple Silicon)
mac-arm64:
	@echo "Building macOS ARM64 release..."
	@mkdir -p $(RELEASE_DIR)
	cargo build --release --target $(TARGET_MAC_ARM64)
	cp target/$(TARGET_MAC_ARM64)/release/$(BINARY_NAME) $(BIN_MAC_ARM64)
	@echo "✓ macOS ARM64 binary: $(BIN_MAC_ARM64)"

# Build macOS x64 (Intel)
mac-x64:
	@echo "Building macOS x64 release..."
	@mkdir -p $(RELEASE_DIR)
	cargo build --release --target $(TARGET_MAC_X64)
	cp target/$(TARGET_MAC_X64)/release/$(BINARY_NAME) $(BIN_MAC_X64)
	@echo "✓ macOS x64 binary: $(BIN_MAC_X64)"

# Build Linux x64
# Uses 'cross' for cross-compilation from macOS (requires Docker)
linux:
	@echo "Building Linux x64 release..."
	@if ! command -v docker &> /dev/null && ! command -v podman &> /dev/null; then \
		echo "⚠️  Docker or Podman required for Linux cross-compilation."; \
		echo "    Install Docker Desktop from: https://www.docker.com/products/docker-desktop"; \
		echo "    Or build on a Linux machine using: cargo build --release --target x86_64-unknown-linux-gnu"; \
		echo "✗ Skipping Linux build"; \
	else \
		if ! command -v cross &> /dev/null; then \
			echo "⚠️  'cross' not found. Installing..."; \
			cargo install cross --git https://github.com/cross-rs/cross || exit 1; \
		fi; \
		mkdir -p $(RELEASE_DIR); \
		echo "Using 'cross' for Linux cross-compilation..."; \
		cross build --release --target $(TARGET_LINUX_X64); \
		cp target/$(TARGET_LINUX_X64)/release/$(BINARY_NAME) $(BIN_LINUX_X64); \
		echo "✓ Linux x64 binary: $(BIN_LINUX_X64)"; \
	fi

# Build Windows x64
windows:
	@echo "Building Windows x64 release..."
	@mkdir -p $(RELEASE_DIR)
	cargo build --release --target $(TARGET_WINDOWS_X64)
	cp target/$(TARGET_WINDOWS_X64)/release/$(BINARY_NAME).exe $(BIN_WINDOWS_X64)
	@echo "✓ Windows x64 binary: $(BIN_WINDOWS_X64)"

# Install all required Rust targets
install-targets:
	@echo "Installing Rust targets..."
	rustup target add $(TARGET_MAC_ARM64)
	rustup target add $(TARGET_MAC_X64)
	rustup target add $(TARGET_LINUX_X64)
	rustup target add $(TARGET_WINDOWS_X64)
	@echo "✓ All targets installed"

# Create GitHub release and upload binaries
github-release: release
	@echo "Creating GitHub release v$(VERSION)..."
	@if ! command -v gh &> /dev/null; then \
		echo "✗ Error: GitHub CLI (gh) not installed. Install with: brew install gh"; \
		exit 1; \
	fi
	@echo "Checking authentication..."
	@gh auth status || (echo "✗ Not authenticated. Run: gh auth login" && exit 1)
	@echo "Collecting available binaries..."
	@BINARIES=""; \
	if [ -f "$(BIN_MAC_ARM64)" ]; then BINARIES="$$BINARIES $(BIN_MAC_ARM64)"; fi; \
	if [ -f "$(BIN_MAC_X64)" ]; then BINARIES="$$BINARIES $(BIN_MAC_X64)"; fi; \
	if [ -f "$(BIN_LINUX_X64)" ]; then BINARIES="$$BINARIES $(BIN_LINUX_X64)"; fi; \
	if [ -f "$(BIN_WINDOWS_X64)" ]; then BINARIES="$$BINARIES $(BIN_WINDOWS_X64)"; fi; \
	echo "Binaries to upload:$$BINARIES"; \
	echo "Creating release v$(VERSION)..."; \
	gh release create v$(VERSION) \
		--title "Roon Remote Display v$(VERSION)" \
		--notes "Release v$(VERSION) - Roon Remote Display\n\n## Binaries\n- macOS ARM64 (Apple Silicon)\n- macOS x64 (Intel)\n- Linux x64 (if available)\n- Windows x64\n\n## Installation\nDownload the appropriate binary for your platform and run it." \
		$$BINARIES
	@echo "✓ GitHub release v$(VERSION) created successfully!"
	@echo "  View at: https://github.com/jdrivas/roon-rd/releases/tag/v$(VERSION)"

# Clean build artifacts
clean:
	@echo "Cleaning build artifacts..."
	cargo clean
	rm -rf release/
	@echo "✓ Clean complete"
