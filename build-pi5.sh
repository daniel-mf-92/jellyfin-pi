#!/usr/bin/env bash
set -euo pipefail

# Jellyfin TV - Build Script for Raspberry Pi 5
# Builds the Slint + Rust Jellyfin TV client natively on ARM64

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_TYPE="${1:-release}"

echo "=== Jellyfin TV Build Script ==="
echo "Target: Raspberry Pi 5 (ARM64)"
echo "Build type: ${BUILD_TYPE}"

# 1. Check/install system dependencies
install_deps() {
    echo "--- Checking system dependencies ---"
    
    # Core build tools
    sudo apt-get update
    sudo apt-get install -y \
        build-essential \
        pkg-config \
        cmake \
        curl \
        git \
        libssl-dev \
        libgl1-mesa-dev \
        libegl1-mesa-dev \
        libgles2-mesa-dev \
        libgbm-dev \
        libdrm-dev \
        libinput-dev \
        libudev-dev \
        libxkbcommon-dev \
        libseat-dev \
        libmpv-dev \
        libasound2-dev \
        libfontconfig-dev \
        libfreetype-dev
    
    echo "System dependencies installed."
}

# 2. Check/install Rust toolchain
install_rust() {
    if ! command -v rustc &> /dev/null; then
        echo "--- Installing Rust toolchain ---"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        source "$HOME/.cargo/env"
    fi
    
    echo "Rust version: $(rustc --version)"
    echo "Cargo version: $(cargo --version)"
    
    # Ensure we have the right target
    rustup target add aarch64-unknown-linux-gnu 2>/dev/null || true
}

# 3. Build
build() {
    cd "$SCRIPT_DIR"
    
    echo "--- Building Jellyfin TV ---"
    
    # Set environment for Slint linuxkms backend
    export SLINT_BACKEND=linuxkms
    
    if [ "$BUILD_TYPE" = "release" ]; then
        cargo build --release
        BINARY="target/release/jellyfin-tv"
    else
        cargo build
        BINARY="target/debug/jellyfin-tv"
    fi
    
    if [ -f "$BINARY" ]; then
        echo "=== Build successful ==="
        echo "Binary: $BINARY"
        echo "Size: $(du -h "$BINARY" | cut -f1)"
        echo ""
        echo "To install: sudo cp $BINARY /usr/local/bin/"
        echo "Or run: ./install.sh"
    else
        echo "!!! Build failed !!!"
        exit 1
    fi
}

# Run
install_deps
install_rust
build
